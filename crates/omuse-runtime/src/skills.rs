//! Skills: SKILL.md files the agent can load (name + description in the prompt, full text on load_skill).
//!
//! Two places: the built-in ones shipped in `skills/` (compiled into the binary and extracted to
//! `<runtime data>/skills-builtin` at start, unless `[skills] dir` names a directory) and the ones the user imported
//! from GitHub, kept in `<runtime data>/skills` so they survive upgrades. Which ones the agent sees is Settings → Skills
//! (`skills_disabled`), a list of names that are off. The list is written once, at the first start of a fresh database:
//! from `[skills] initial` (names that are on; everything else off) or empty (everything on) when the key is unset.
//! After that the database decides and the config value is ignored. A skill that is off is not listed in the prompt and
//! load_skill refuses it; a skill imported later is on (it is not in the list).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::Result;
use include_dir::{include_dir, Dir, DirEntry};
use omuse_common::util::get_str;
use omuse_common::Config;
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::store::Store;

/// The built-in skills (`skills/*`), compiled in.
pub static BUILTIN: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../skills");

pub const MAX_BYTES: usize = 256 * 1024;
static NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9._-]{0,63}$").unwrap());

/// `SkillError`: a message for the user (HTTP 400 in the API).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct SkillError(pub String);

/// Extract the embedded built-in skills to `<runtime_data>/skills-builtin` (files written or refreshed; skill folders
/// that are no longer shipped are removed) and return that directory. Failures are printed, never fatal.
pub fn builtin_dir(runtime_data: &Path) -> PathBuf {
    let target = runtime_data.join("skills-builtin");
    fn extract(dir: &Dir<'_>, target: &Path) {
        for entry in dir.entries() {
            match entry {
                DirEntry::Dir(d) => extract(d, target),
                DirEntry::File(f) => {
                    let p = target.join(f.path());
                    let same = std::fs::read(&p)
                        .map(|cur| cur == f.contents())
                        .unwrap_or(false);
                    if same {
                        continue;
                    }
                    let res = p
                        .parent()
                        .map(std::fs::create_dir_all)
                        .unwrap_or(Ok(()))
                        .and_then(|_| std::fs::write(&p, f.contents()));
                    if let Err(e) = res {
                        println!("[skills] cannot write {}: {}", p.display(), e);
                    }
                }
            }
        }
    }
    if let Err(e) = std::fs::create_dir_all(&target) {
        println!("[skills] cannot create {}: {}", target.display(), e);
        return target;
    }
    extract(&BUILTIN, &target);
    let shipped: HashSet<String> = BUILTIN
        .dirs()
        .filter_map(|d| {
            d.path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .collect();
    if let Ok(rd) = std::fs::read_dir(&target) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() && !shipped.contains(&name) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    target
}

/// name / description (and `browser`, a skill's browser policy) from the YAML front matter (`--- ... ---`) at the top
/// of a SKILL.md; empty when there is none.
pub fn front_matter(text: &str) -> HashMap<String, String> {
    static FM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)^---\s*\n(.*?)\n---\s*(\n|$)").unwrap());
    let mut out = HashMap::new();
    let Some(m) = FM.captures(text) else {
        return out;
    };
    for line in m[1].lines() {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim();
            if matches!(k, "name" | "description" | "browser" | "plan") {
                out.insert(
                    k.to_string(),
                    v.trim().trim_matches(|c| c == '"' || c == '\'').to_string(),
                );
            }
        }
    }
    out
}

fn read_text(p: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(p)?;
    Ok(String::from_utf8_lossy(&bytes)
        .replace("\r\n", "\n")
        .replace('\r', "\n"))
}

/// What `[skills] initial` leaves off: every skill not named in it; nothing when the key is unset or empty.
pub fn env_disabled(names: &[String], initial: Option<&[String]>) -> Vec<String> {
    let on: HashSet<String> = initial
        .unwrap_or(&[])
        .iter()
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect();
    if on.is_empty() {
        return vec![];
    }
    names.iter().filter(|n| !on.contains(*n)).cloned().collect()
}

/// The saved selection (Settings → Skills); an unseeded database (tests, old installs before `seed()`) means all on.
pub fn disabled_names(settings: &Value) -> Vec<String> {
    match settings.get("skills_disabled") {
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => s.clone(),
                other => omuse_common::util::py_str(other),
            })
            .collect(),
        _ => vec![],
    }
}

/// The skill folders of one install: built-in, imported, and where GitHub raw files come from.
#[derive(Debug, Clone)]
pub struct Skills {
    /// `BUILTIN_DIR` (`[skills] dir`, or the extracted built-ins).
    pub builtin_dir: PathBuf,
    /// `USER_DIR`: `<runtime data>/skills`.
    pub user_dir: PathBuf,
    /// `GITHUB_RAW` (no trailing slash).
    pub github_raw: String,
}

impl Skills {
    /// From the config: `[skills] dir` when set, else the built-ins extracted under the runtime data directory.
    pub fn new(cfg: &Config) -> Skills {
        let runtime_data = cfg.runtime_data();
        let builtin = if cfg.skills.dir.is_empty() {
            builtin_dir(&runtime_data)
        } else {
            PathBuf::from(&cfg.skills.dir)
        };
        Skills::with_dirs(
            builtin,
            runtime_data.join("skills"),
            &cfg.endpoints.github_raw,
        )
    }

    pub fn with_dirs(builtin_dir: PathBuf, user_dir: PathBuf, github_raw: &str) -> Skills {
        Skills {
            builtin_dir,
            user_dir,
            github_raw: github_raw.trim_end_matches('/').to_string(),
        }
    }

    fn scan(folder: &Path, source: &str) -> Vec<Value> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(folder) else {
            return out;
        };
        let mut names: Vec<String> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in names {
            let p = folder.join(&name).join("SKILL.md");
            if p.is_file() {
                let Ok(txt) = read_text(&p) else {
                    continue;
                };
                let fm = front_matter(&txt);
                out.push(json!({
                    "name": name,
                    "description": fm.get("description").cloned().unwrap_or_default(),
                    "path": p.to_string_lossy(),
                    "source": source,
                }));
            }
        }
        out
    }

    /// Built-in skills, then imported ones; an imported skill with a built-in's name is not possible (import refuses).
    pub fn all_skills(&self) -> Vec<Value> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::new();
        for s in Self::scan(&self.builtin_dir, "builtin")
            .into_iter()
            .chain(Self::scan(&self.user_dir, "imported"))
        {
            if seen.insert(get_str(&s, "name")) {
                out.push(s);
            }
        }
        out
    }

    /// First start of a fresh database: write the initial selection from `[skills] initial` (or "everything on").
    /// Returns the list written, or `None` when the database already had one (then the config value is ignored).
    pub fn seed(&self, store: &Store, initial: Option<Vec<String>>) -> Result<Option<Vec<String>>> {
        if store
            .settings()?
            .get("skills_disabled")
            .map(Value::is_array)
            .unwrap_or(false)
        {
            return Ok(None);
        }
        let names: Vec<String> = self
            .all_skills()
            .iter()
            .map(|s| get_str(s, "name"))
            .collect();
        let off = env_disabled(&names, initial.as_deref());
        store.set_settings(&json!({"skills_disabled": off}))?;
        Ok(Some(off))
    }

    /// Every skill with its `enabled` flag.
    pub fn listing(&self, settings: &Value) -> Vec<Value> {
        let off: HashSet<String> = disabled_names(settings).into_iter().collect();
        self.all_skills()
            .into_iter()
            .map(|s| {
                let mut m: Map<String, Value> = s.as_object().cloned().unwrap_or_default();
                m.insert("enabled".into(), json!(!off.contains(&get_str(&s, "name"))));
                Value::Object(m)
            })
            .collect()
    }

    /// The skills the agent may use.
    pub fn enabled(&self, settings: &Value) -> Vec<Value> {
        self.listing(settings)
            .into_iter()
            .filter(|s| s.get("enabled").and_then(Value::as_bool).unwrap_or(false))
            .collect()
    }

    /// The raw SKILL.md address for a GitHub link: a repo (SKILL.md at its root, default branch), a folder
    /// (…/tree/<branch>/<path>), a file (…/blob/<branch>/<path>/SKILL.md) or a raw.githubusercontent.com address.
    pub fn raw_url(&self, link: &str) -> std::result::Result<String, SkillError> {
        let link = link.trim();
        let u = url::Url::parse(link)
            .ok()
            .filter(|u| u.scheme() == "http" || u.scheme() == "https")
            .ok_or_else(|| SkillError("请输入 GitHub 链接 (enter a GitHub link)".into()))?;
        let host = u.host_str().unwrap_or("").to_lowercase();
        let parts: Vec<&str> = u.path().split('/').filter(|p| !p.is_empty()).collect();
        if host == "raw.githubusercontent.com" {
            return Ok(if self.github_raw != "https://raw.githubusercontent.com" {
                format!("{}{}", self.github_raw, u.path())
            } else {
                link.to_string()
            });
        }
        if (host != "github.com" && host != "www.github.com") || parts.len() < 2 {
            return Err(SkillError(
                "只支持 github.com 上的仓库或目录链接 (only github.com repository / folder links are supported)".into(),
            ));
        }
        let owner = parts[0];
        let repo = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
        if parts.len() >= 4 && (parts[2] == "tree" || parts[2] == "blob") {
            let branch = parts[3];
            let mut rest: Vec<&str> = parts[4..].to_vec();
            if rest
                .last()
                .map(|r| r.to_uppercase() == "SKILL.MD")
                .unwrap_or(false)
            {
                rest.pop();
            }
            rest.push("SKILL.md");
            return Ok(format!(
                "{}/{}/{}/{}/{}",
                self.github_raw,
                owner,
                repo,
                branch,
                rest.join("/")
            ));
        }
        Ok(format!(
            "{}/{}/{}/HEAD/SKILL.md",
            self.github_raw, owner, repo
        ))
    }

    /// Fetch the SKILL.md a GitHub link points at and save it under the user's skills; returns the listing entry.
    pub async fn import_from_github(&self, link: &str) -> std::result::Result<Value, SkillError> {
        let url = self.raw_url(link)?;
        let client = omuse_common::http::client(30.0);
        let r = client
            .get(&url)
            .header("Accept", "text/plain")
            .send()
            .await
            .map_err(|e| {
                SkillError(format!(
                    "下载失败 download failed: {}",
                    reqwest_error_name(&e)
                ))
            })?;
        let status = r.status().as_u16();
        if status == 404 {
            return Err(SkillError(
                "这个链接下没有 SKILL.md (no SKILL.md at that link: give the repository or the skill's folder)".into(),
            ));
        }
        if status >= 400 {
            return Err(SkillError(format!(
                "下载失败 download failed: HTTP {}",
                status
            )));
        }
        let content = r.bytes().await.map_err(|e| {
            SkillError(format!(
                "下载失败 download failed: {}",
                reqwest_error_name(&e)
            ))
        })?;
        if content.len() > MAX_BYTES {
            return Err(SkillError("SKILL.md 太大 (file too large)".into()));
        }
        let text = String::from_utf8_lossy(&content).into_owned();
        let fm = front_matter(&text);
        let name = fm
            .get("name")
            .map(|n| n.trim().to_lowercase())
            .unwrap_or_default();
        let description = fm.get("description").cloned().unwrap_or_default();
        if name.is_empty() || description.is_empty() {
            return Err(SkillError(
                "SKILL.md 需要 name 和 description 两个字段 (the front matter needs name and description)".into(),
            ));
        }
        if !NAME_RE.is_match(&name) {
            return Err(SkillError(
                "技能名只能用小写字母、数字、. _ - (name: lowercase letters, digits, . _ -)".into(),
            ));
        }
        if self
            .all_skills()
            .iter()
            .any(|s| get_str(s, "name") == name && get_str(s, "source") == "builtin")
        {
            return Err(SkillError(format!(
                "已有内置技能叫 {} (a built-in skill has that name)",
                name
            )));
        }
        let folder = self.user_dir.join(&name);
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(&folder)?;
            std::fs::write(folder.join("SKILL.md"), text.as_bytes())?;
            std::fs::write(
                folder.join("SOURCE"),
                format!("{}\n", link.trim()).as_bytes(),
            )
        };
        write().map_err(|e| SkillError(format!("cannot save the skill: {}", e)))?;
        Ok(
            json!({"name": name, "description": description, "source": "imported", "url": link.trim()}),
        )
    }

    /// Remove an imported skill's folder; `false` for a built-in or unknown name.
    pub fn delete_imported(&self, name: &str) -> bool {
        let folder = self.user_dir.join(name);
        if !NAME_RE.is_match(name) || !folder.join("SKILL.md").is_file() {
            return false;
        }
        let _ = std::fs::remove_dir_all(&folder);
        true
    }

    /// The GitHub link an imported skill came from ("" when unknown).
    pub fn source_link(&self, name: &str) -> String {
        read_text(&self.user_dir.join(name).join("SOURCE"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }
}

/// Python's `type(e).__name__` for an httpx transport error, approximated from reqwest's error kinds.
fn reqwest_error_name(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        if e.is_connect() {
            "ConnectTimeout"
        } else {
            "ReadTimeout"
        }
    } else if e.is_connect() {
        "ConnectError"
    } else if e.is_redirect() {
        "TooManyRedirects"
    } else {
        "TransportError"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_parsing() {
        let fm = front_matter(
            "---\nname: Flight-Watch\ndescription: \"Watch a flight\"\nother: x\n---\n# Body\n",
        );
        assert_eq!(fm.get("name").unwrap(), "Flight-Watch");
        assert_eq!(fm.get("description").unwrap(), "Watch a flight");
        assert!(!fm.contains_key("other"));
        let fm = front_matter("---\nname: y\nbrowser: off\nplan: on\n---\n");
        assert_eq!(fm.get("browser").unwrap(), "off");
        assert_eq!(fm.get("plan").unwrap(), "on");
        assert!(front_matter("# No front matter\n").is_empty());
        assert!(front_matter("---\nname: x\n").is_empty());
        let fm = front_matter("---\nname: x\n---");
        assert_eq!(fm.get("name").unwrap(), "x");
    }

    #[test]
    fn raw_urls() {
        let sk = Skills::with_dirs(
            PathBuf::from("/nonexistent/a"),
            PathBuf::from("/nonexistent/b"),
            "http://127.0.0.1:9/raw/",
        );
        assert_eq!(
            sk.raw_url("https://github.com/acme/skills/tree/main/flight-watch")
                .unwrap(),
            "http://127.0.0.1:9/raw/acme/skills/main/flight-watch/SKILL.md"
        );
        assert_eq!(
            sk.raw_url("https://github.com/acme/skills/blob/main/flight-watch/SKILL.md")
                .unwrap(),
            "http://127.0.0.1:9/raw/acme/skills/main/flight-watch/SKILL.md"
        );
        assert_eq!(
            sk.raw_url("https://github.com/acme/skills.git").unwrap(),
            "http://127.0.0.1:9/raw/acme/skills/HEAD/SKILL.md"
        );
        assert_eq!(
            sk.raw_url("https://raw.githubusercontent.com/acme/skills/main/x/SKILL.md")
                .unwrap(),
            "http://127.0.0.1:9/raw/acme/skills/main/x/SKILL.md"
        );
        let real = Skills::with_dirs(
            PathBuf::from("/x"),
            PathBuf::from("/y"),
            "https://raw.githubusercontent.com",
        );
        assert_eq!(
            real.raw_url("https://raw.githubusercontent.com/acme/skills/main/x/SKILL.md")
                .unwrap(),
            "https://raw.githubusercontent.com/acme/skills/main/x/SKILL.md"
        );
        assert!(sk
            .raw_url("https://gitlab.com/acme/skills")
            .unwrap_err()
            .0
            .contains("github.com"));
        assert!(sk
            .raw_url("ftp://github.com/a/b")
            .unwrap_err()
            .0
            .contains("GitHub"));
        assert!(sk.raw_url("https://github.com/acme").is_err());
    }

    #[test]
    fn builtins_extract_and_list() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("skills-builtin/stale-old")).unwrap();
        let dir = builtin_dir(d.path());
        assert_eq!(dir, d.path().join("skills-builtin"));
        assert!(dir.join("reply-email/SKILL.md").is_file());
        assert!(!dir.join("stale-old").exists());
        let user = d.path().join("skills");
        let sk = Skills::with_dirs(dir, user.clone(), "https://raw.githubusercontent.com");
        let all = sk.all_skills();
        assert!(all.len() >= 9);
        assert!(all
            .iter()
            .all(|s| get_str(s, "source") == "builtin" && !get_str(s, "description").is_empty()));
        let names: Vec<String> = all.iter().map(|s| get_str(s, "name")).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        // an imported one
        std::fs::create_dir_all(user.join("flight-watch")).unwrap();
        std::fs::write(
            user.join("flight-watch/SKILL.md"),
            "---\nname: flight-watch\ndescription: Flight watch\n---\n",
        )
        .unwrap();
        std::fs::write(
            user.join("flight-watch/SOURCE"),
            "https://github.com/acme/skills/tree/main/flight-watch\n",
        )
        .unwrap();
        assert_eq!(
            sk.source_link("flight-watch"),
            "https://github.com/acme/skills/tree/main/flight-watch"
        );
        assert_eq!(sk.source_link("reply-email"), "");
        let settings = json!({"skills_disabled": ["online-shopping", "phone-call"]});
        let listing = sk.listing(&settings);
        let off: Vec<String> = listing
            .iter()
            .filter(|s| s["enabled"] == json!(false))
            .map(|s| get_str(s, "name"))
            .collect();
        assert_eq!(off, vec!["online-shopping", "phone-call"]);
        assert!(listing.iter().any(|s| get_str(s, "name") == "flight-watch"
            && get_str(s, "source") == "imported"
            && s["enabled"] == json!(true)));
        assert_eq!(sk.enabled(&settings).len(), listing.len() - 2);
        assert_eq!(sk.enabled(&json!({})).len(), listing.len());
        assert!(!sk.delete_imported("reply-email"));
        assert!(sk.delete_imported("flight-watch"));
        assert!(!user.join("flight-watch").exists());
        // seeding
        let st = Store::new(d.path(), std::sync::Arc::new(Config::parse("").unwrap())).unwrap();
        assert_eq!(
            sk.seed(&st, Some(vec!["reply-email".into()]))
                .unwrap()
                .unwrap()
                .len(),
            names.len() - 1
        );
        assert_eq!(sk.seed(&st, None).unwrap(), None);
        assert_eq!(env_disabled(&names, None), Vec::<String>::new());
        assert_eq!(env_disabled(&names, Some(&[])), Vec::<String>::new());
        assert_eq!(
            disabled_names(&json!({"skills_disabled": "x"})),
            Vec::<String>::new()
        );
    }
}
