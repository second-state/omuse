//! The two HTML pages, as Tera templates compiled into the binary.
//!
//! * `templates/index.html` is `web/index.html` with every `static/<file>` URL stamped `?v={{ v }}`
//!   (what the Python `Assets.stamp` regex did per request). The DOM is otherwise identical — the Playwright UI
//!   tests depend on it.
//! * `templates/login.html` is the `LOGIN_HTML` of `deploy/docker/gate.py`. Autoescape is on (Tera's default for
//!   `.html` names) and uses Python's `html.escape` character table, so `user` / `next` render byte-for-byte as
//!   the Python page did; `error` is pre-rendered markup and marked safe.

use tera::{Context, Tera};

/// `templates/index.html` (the page shell with `?v={{ v }}` stamps).
pub const INDEX_HTML: &str = include_str!("../templates/index.html");
/// `templates/login.html` (the gate's login page).
pub const LOGIN_HTML: &str = include_str!("../templates/login.html");

/// Python `html.escape(s, quote=True)`: `&`, `<`, `>`, `"`, `'`.
pub fn py_html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

/// The values of the login page (`BasicAuthGate._page`).
#[derive(Debug, Clone, Default)]
pub struct LoginPage<'a> {
    /// `lang` attribute: `zh-CN` or `en`.
    pub lang: &'a str,
    pub title: &'a str,
    pub user_label: &'a str,
    pub pw_label: &'a str,
    pub button: &'a str,
    pub hint: &'a str,
    /// The error text (plain; escaped and wrapped in `<p class="err">` here), or empty for none.
    pub error: &'a str,
    /// Pre-filled user name (escaped by the template).
    pub user: &'a str,
    /// The `next` URL carried through the form (escaped by the template).
    pub next: &'a str,
}

/// The compiled templates; build once and share.
#[derive(Debug)]
pub struct Templates {
    tera: Tera,
}

impl Default for Templates {
    fn default() -> Self {
        Self::new()
    }
}

impl Templates {
    /// Compile `index.html` and `login.html`.
    pub fn new() -> Self {
        let mut tera = Tera::default();
        tera.set_escape_fn(py_html_escape);
        tera.add_raw_templates([("index.html", INDEX_HTML), ("login.html", LOGIN_HTML)])
            .expect("the embedded templates compile");
        Self { tera }
    }

    /// The page shell with every asset URL stamped `?v=<v>`.
    pub fn render_index(&self, v: &str) -> String {
        let mut ctx = Context::new();
        ctx.insert("v", v);
        self.tera
            .render("index.html", &ctx)
            .expect("index.html renders")
    }

    /// The login page.
    pub fn render_login(&self, p: &LoginPage<'_>) -> String {
        let mut ctx = Context::new();
        ctx.insert("lang", p.lang);
        ctx.insert("title", p.title);
        ctx.insert("user_label", p.user_label);
        ctx.insert("pw_label", p.pw_label);
        ctx.insert("button", p.button);
        ctx.insert("hint", p.hint);
        let err = if p.error.is_empty() {
            String::new()
        } else {
            format!("<p class=\"err\">{}</p>", py_html_escape(p.error))
        };
        ctx.insert("error", &err);
        ctx.insert("user", p.user);
        ctx.insert("next", p.next);
        self.tera
            .render("login.html", &ctx)
            .expect("login.html renders")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{stamp, INDEX, WEB};

    #[test]
    fn index_is_the_stamped_original() {
        let t = Templates::new();
        let original = WEB.get_file(INDEX).unwrap().contents_utf8().unwrap();
        let v = "0123456789ab";
        let rendered = t.render_index(v);
        assert_eq!(rendered, stamp(original, v));
        assert!(
            rendered.contains(r#"<html lang="zh-CN">"#)
                && rendered.contains("<title>OMuse</title>")
        );
        assert!(rendered.contains(r#"href="static/app.css?v=0123456789ab""#));
        assert!(rendered.contains(r#"<script src="static/app.js?v=0123456789ab"></script>"#));
        assert!(!rendered.contains("{{") && !rendered.contains("}}"));
        assert_eq!(rendered.matches("?v=").count(), 6);
    }

    #[test]
    fn login_escapes_like_python() {
        let t = Templates::new();
        let page = t.render_login(&LoginPage {
            lang: "en",
            title: "Sign in",
            user_label: "User",
            pw_label: "Password",
            button: "Sign in",
            hint: "This browser stays signed in; changing the password signs every browser out.",
            error: "",
            user: "om\"use<b>&'",
            next: "/#settings",
        });
        assert!(page.starts_with("<!doctype html>\n<html lang=\"en\"><head>"));
        assert!(page.ends_with("</form></body></html>"));
        assert!(page.contains(
            r#"<input name="user" value="om&quot;use&lt;b&gt;&amp;&#x27;" autocomplete="username""#
        ));
        assert!(page.contains(r#"<input type="hidden" name="next" value="/#settings">"#));
        assert!(page.contains("<h1>OMuse <span>Sign in</span></h1>\n\n<label>User<input"));
        let page = t.render_login(&LoginPage {
            error: "Wrong <user> or password",
            user: "omuse",
            next: "/",
            ..Default::default()
        });
        assert!(
            page.contains("</h1>\n<p class=\"err\">Wrong &lt;user&gt; or password</p>\n<label>")
        );
    }

    #[test]
    fn escape_table() {
        assert_eq!(
            py_html_escape("a&b<c>d\"e'f/g"),
            "a&amp;b&lt;c&gt;d&quot;e&#x27;f/g"
        );
        assert_eq!(py_html_escape("登录"), "登录");
    }
}
