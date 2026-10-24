//! Chromium discovery and launch configuration, and the Xvfb virtual display (`ensure_pw` / `ensure_ctx` /
//! `_start_xvfb` in the Python broker).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use chromiumoxide::browser::BrowserConfig;
use chromiumoxide::handler::viewport::Viewport;
use omuse_common::Config;

use crate::textfmt::{VIEWPORT_H, VIEWPORT_W};

/// Candidates on `PATH` (the Dockerfiles install one of them).
const PATH_NAMES: &[&str] = &[
    "chromium",
    "chromium-browser",
    "google-chrome",
    "google-chrome-stable",
];

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn glob_first(dir: &Path, prefix: &str, tail: &[&str]) -> Option<PathBuf> {
    let mut hits: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .map(|e| e.path())
        .collect();
    hits.sort();
    for h in hits.into_iter().rev() {
        for t in tail {
            let p = h.join(t);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// The Chromium binary: `[browser] executable`, else `chromium` / `chromium-browser` / `google-chrome` on `PATH`,
/// Playwright's bundle under `/ms-playwright`, and the usual macOS locations for local development.
pub fn find_executable(cfg: &Config) -> Option<PathBuf> {
    let exe = cfg.browser.executable.trim();
    if !exe.is_empty() {
        return Some(PathBuf::from(exe));
    }
    for n in PATH_NAMES {
        if let Some(p) = which(n) {
            return Some(p);
        }
    }
    if let Some(p) = glob_first(
        Path::new("/ms-playwright"),
        "chromium-",
        &["chrome-linux/chrome", "chrome-linux64/chrome"],
    ) {
        return Some(p);
    }
    for mac in [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ] {
        if Path::new(mac).is_file() {
            return Some(PathBuf::from(mac));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let cache = Path::new(&home).join("Library/Caches/ms-playwright");
        if let Some(p) = glob_first(
            &cache,
            "chromium-",
            &["chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
              "chrome-mac/Chromium.app/Contents/MacOS/Chromium"],
        ) {
            return Some(p);
        }
    }
    None
}

/// Start `Xvfb <display> -screen 0 1280x900x24 -nolisten tcp -ac` and wait for its socket (`_start_xvfb`).
/// `None` when Xvfb is not installed or does not come up.
pub fn start_xvfb(display: &str) -> Option<Child> {
    which("Xvfb")?;
    let n = display.trim_start_matches(':');
    for f in [
        format!("/tmp/.X{}-lock", n),
        format!("/tmp/.X11-unix/X{}", n),
    ] {
        let _ = std::fs::remove_file(f);
    }
    let mut child = Command::new("Xvfb")
        .args([
            display,
            "-screen",
            "0",
            "1280x900x24",
            "-nolisten",
            "tcp",
            "-ac",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let sock = format!("/tmp/.X11-unix/X{}", n);
    for _ in 0..50 {
        if Path::new(&sock).exists() {
            return Some(child);
        }
        if let Ok(Some(_)) = child.try_wait() {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    None
}

/// Chromium switches: Playwright's launcher defaults without `--enable-automation` (the Python passed
/// `ignore_default_args=["--enable-automation"]`), plus the flags the broker added. Site isolation is turned off so
/// cross-origin iframes (card fields, chat widgets) live in the page's renderer and can be read through one session.
fn common_args() -> Vec<String> {
    [
        "--disable-field-trial-config",
        "--disable-background-networking",
        "--disable-background-timer-throttling",
        "--disable-backgrounding-occluded-windows",
        "--disable-back-forward-cache",
        "--disable-breakpad",
        "--disable-client-side-phishing-detection",
        "--disable-component-extensions-with-background-pages",
        "--disable-component-update",
        "--no-default-browser-check",
        "--disable-default-apps",
        "--disable-dev-shm-usage",
        "--disable-extensions",
        "--disable-features=ImprovedCookieControls,LazyFrameLoading,GlobalMediaControls,DestroyProfileOnBrowserClose,MediaRouter,DialMediaRouteProvider,AcceptCHFrame,AutoExpandDetailsElement,CertificateTransparencyComponentUpdater,AvoidUnnecessaryBeforeUnloadCheckSync,Translate,HttpsUpgrades,PaintHolding,ThirdPartyStoragePartitioning,LensOverlay,IsolateOrigins,site-per-process",
        "--disable-site-isolation-trials",
        "--allow-pre-commit-input",
        "--disable-hang-monitor",
        "--disable-ipc-flooding-protection",
        "--disable-popup-blocking",
        "--disable-prompt-on-repost",
        "--disable-renderer-backgrounding",
        "--force-color-profile=srgb",
        "--metrics-recording-only",
        "--no-first-run",
        "--password-store=basic",
        "--use-mock-keychain",
        "--no-service-autorun",
        "--export-tagged-pdf",
        "--disable-search-engine-choice-screen",
        "--unsafely-disable-devtools-self-xss-warnings",
        "--no-sandbox",
        "--disable-setuid-sandbox",
        "--disable-blink-features=AutomationControlled",
        "--lang=en-US",
    ]
    .iter()
    .map(|s| flag(s))
    .collect()
}

/// chromiumoxide writes `--<key>` itself: strip our leading dashes so `--no-sandbox` does not become `----no-sandbox`
/// (which Chromium silently ignores).
fn flag(s: &str) -> String {
    s.trim_start_matches('-').to_string()
}

/// Launch configuration of the persistent (user-profile) Chromium.
pub fn persistent_config(
    exe: &Path,
    profile: &Path,
    headless: bool,
    display: &str,
) -> Result<BrowserConfig, String> {
    let mut b = BrowserConfig::builder()
        .chrome_executable(exe)
        .user_data_dir(profile)
        .disable_default_args()
        .args(common_args())
        .arg(flag("--window-size=1280,900"))
        .viewport(Viewport {
            width: VIEWPORT_W,
            height: VIEWPORT_H,
            device_scale_factor: Some(1.0),
            ..Default::default()
        })
        .launch_timeout(Duration::from_secs(60))
        .request_timeout(Duration::from_secs(60))
        .disable_request_intercept()
        .enable_cache();
    b = if headless {
        b.new_headless_mode()
    } else {
        b.with_head()
    };
    if !display.is_empty() {
        b = b.env("DISPLAY", display);
    }
    b.build()
}

/// Launch configuration of the small headless Chromium used for PDF / PNG / compose rendering (a throw-away
/// profile; nothing of the user's).
pub fn render_config(exe: &Path, scratch: &Path) -> Result<BrowserConfig, String> {
    BrowserConfig::builder()
        .chrome_executable(exe)
        .user_data_dir(scratch)
        .disable_default_args()
        .args(common_args())
        .new_headless_mode()
        .arg(flag("--hide-scrollbars"))
        .arg(flag("--mute-audio"))
        .viewport(Viewport {
            width: VIEWPORT_W,
            height: VIEWPORT_H,
            device_scale_factor: Some(1.0),
            ..Default::default()
        })
        .launch_timeout(Duration::from_secs(60))
        .request_timeout(Duration::from_secs(60))
        .disable_request_intercept()
        .build()
}
