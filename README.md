<p align="center"><img src="deploy/market/assets/icon.png" width="96" alt="OMuse"></p>

<h1 align="center">OMuse</h1>
<p align="center"><b>Your private AI agent that lives on your Olares — and asks before it acts.</b><br>
运行在你 Olares 上的私人 AI 智能体——重要的事，先问你。</p>

![OMuse](deploy/market/assets/featured.webp)

[English](#english) · [中文](#中文)

---

## English

OMuse is a local-first AI agent for [Olares](https://www.olares.com). It doesn't just chat: it turns a request into a plan and carries it out across your email, Notion, Slack and the web. Anything that sends, posts, pays or deletes waits for your approval.

### Features

- **Chat → plan → act** – every step and tool call is visible.
- **Email** (several mailboxes: Gmail, Outlook / Hotmail via OAuth 2.0, Yahoo, iCloud, QQ Mail, NetEase 163 / 126, Zoho, AOL, any IMAP / SMTP server) – search, read, draft, reply, send, one-click unsubscribe. Sign-in codes and reset links are hidden from the model.
- **Notion & Slack** – look things up, write reports, update boards, read and post messages.
- **Web browser** – its own Chromium with a restricted API, live view, and human takeover for logins/CAPTCHAs.
- **MCP connectors** – plug in any MCP (Model Context Protocol) server; tool definitions are pinned.
- **Automations** – multi-day goals, event triggers (new email / Slack message / Notion change) and schedules.
- **Telegram remote control** – give tasks and approve actions from your phone.
- **Research library** – give it a subject; it researches in depth, writes a Markdown report you refine in that subject's chat and refresh later; every report is searchable by the agent.
- **Memory & skills**, **English / 中文 UI**.

### Safe by design

| Layer | What it does |
|---|---|
| **Sentinel** | Independent guard service: ALLOW / DENY / ASK for every action. |
| **Vault** | App passwords and tokens are encrypted in Sentinel; the agent and the model never see them. |
| **Prompt-injection defences** | Emails, pages and tool output are untrusted; suspicious content escalates writes to human approval. |
| **Audit trail** | Append-only, hash-chained log of every model call, tool call, decision and approval. |

### Architecture

```
 browser / Telegram ──► Sentinel :8080  (UI, policy, vault, approvals, audit, connectors)
                            │
                            ├──► Runtime :8081  (planner / executor, scheduler, goals, memory)
                            │        └──► LLM (OpenAI-compatible, default: Olares router)
                            └──► Browser broker :8082  (Chromium over CDP + Xvfb)

 One Rust binary (`omuse`, Axum + Tokio) runs the three services; config.toml holds every setting.
```

### Install

- **Olares Market**: search for *OMuse* (after the listing is approved).
- **Manual**: `sh chart/build.sh omuse` → upload `dist/omuse-<version>.tgz` in Olares Market ▸ *Upload custom chart*. The pod runs the release image (`ghcr.io/second-state/omuse:omuse-v<version>`), one `omuse --role …` per container.

The model endpoint defaults to `https://router.<your-olares-name>.olares.com/v1`; change it in the app's settings or with the `OMUSE_MODEL_URL` / `OMUSE_MODEL` values of the Olares app environment (the chart writes them into the pod's `config.toml`). If the configured model isn't served, OMuse picks an available one.

### Run with Docker

OMuse is a single Rust binary (`omuse`) that runs all three services in one process; state lives in the `omuse-data`
volume. All settings come from one `config.toml` (copy `config.example.toml`; every key is commented and names the
environment variable that sets it in a container without a config file, see below).

```bash
cp config.example.toml config.toml      # set [auth] password, [model] url / name / api_key, timezone
docker build -t omuse .
docker run -d --name omuse --restart unless-stopped \
  -p 8080:8080 -v omuse-data:/omuse -v "$PWD/config.toml:/omuse/config.toml:ro" \
  omuse
```

Open `http://localhost:8080` and sign in as `omuse` (`[auth] user`) with your password. `[auth] password` is only the
initial password: change it in Settings → Password (the UI reminds you until you do); the new one is stored hashed in
the volume and wins from then on. Browsers get a login page and a session cookie (180 days, `[auth] session_days`, or
until the password changes; Settings → Password has Sign out); scripts can send HTTP Basic auth (`curl -u omuse:…`).
Without a password the container refuses to start (exit code 64). Keep `config.toml` out of
git: it holds your password and API key. Before opening the port to the internet, put HTTPS in front (Cloudflare
Tunnel, Tailscale Funnel, Caddy, …). `[model] api_key` is sent as a Bearer token to the model endpoint and is never shown
in Settings; a user may instead switch on "Use my own model" in Settings → Models. Add `-p 8083:8083` only if you use the
Telnyx phone line (`[voice] port`). After editing `config.toml`, `docker restart omuse` (data stays in the volume).

Platforms that hand settings to a container only as environment variables (Fly secrets, Products Provisioner) can
leave the file out: the entrypoint then writes the config for that start from these variables. A mounted `config.toml`
always wins; the binary itself reads only the file.

| Section | Keys (environment variable without a config file) |
|---|---|
| `[auth]` | `password` (`OMUSE_PASSWORD`, required), `user` (`OMUSE_USER`), `enabled = false` (`OMUSE_AUTH=off`), `gate` |
| `[model]` | `url`, `name`, `api_key`, `planner`, `vision`, `stt`, `image`, `image_url`, `image_api_key` (`OMUSE_MODEL_URL`, `OMUSE_MODEL`, …) |
| `[server]` | `port`, `timezone` (`TZ`), … |
| `[skills]` | `initial` (`OMUSE_SKILLS`), `dir` |
| `[telegram]`, `[voice]` | `enabled = false` (`TELEGRAM_BOT=0`), `port = 0` (`VOICE_PORT=0`) |
| `[browser]` | `headless = "1"` (`BROWSER_HEADLESS=1`), `executable` |
| `[stripe]` | `api_key`, `customer_id`, `subscription_id` (`OMUSE_STRIPE_*`) |

The image contains, besides the binary: Chromium and Xvfb (the agent's browser), ffmpeg (video/audio attachments),
`heif-convert` (HEIC photos) and Noto CJK fonts. Nothing is downloaded at run time.

### Deploy on Fly.io

`fly.toml` and `Dockerfile.fly` run the same binary on [Fly.io](https://fly.io): one always-on machine with one volume for
all state, HTTPS from Fly. The image holds no settings: the model endpoint comes from `[env]` in `fly.toml`, the
password and key from Fly secrets (or put a `config.toml` on the volume at `/omuse/config.toml`). It starts as root
only to hand the root-owned volume to the service user.

```bash
fly launch --no-deploy --copy-config        # once: creates the app (choose a name and region)
fly volumes create omuse_data --size 10     # once
fly secrets set OMUSE_PASSWORD='choose-a-long-password' OMUSE_MODEL_API_KEY=sk-...
fly deploy
```

Keep it at one machine: the state is on the volume.

### Publish images for Products Provisioner

`.github/workflows/release-image.yml` builds `Dockerfile.fly` for **linux/amd64**,
tests the resulting image, then pushes that same image to
`ghcr.io/<repository-owner>/locius` (lowercase). Tag releases publish only;
Products Provisioner can import a specified tag using its registry image source.
It does not deploy customer
instances or change their pinned image. No login password, LLM key, or Stripe key
is baked into the image; those are supplied to each instance at runtime.

Publishing uses the workflow's `GITHUB_TOKEN` with `packages: write`; no Fly token,
image-repository app, or organization variable is required. After the first push,
make the GHCR package **public** in its package settings: a public source
repository does not automatically make a new container package public.

Publish this image source in the provisioner's `product.toml`, then use **Sync
image** with the new release tag and review/activate its inherited `image.toml`:

```toml
[image_source]
type = "registry"
repository = "ghcr.io/darumadocker/locius"
```

Use a new release version when switching from Fly's registry: the provisioner
does not allow an existing version to change its image reference. Existing
deployments retain their pinned images.

For an optional manual callback import, set these under GitHub **Settings →
Secrets and variables → Actions**:

| Kind | Name | Value |
|---|---|---|
| Variable | `RELEASE_PRODUCT_KEY` | Product key in the provisioner; defaults to `locius` (independent of the OMuse display name) |
| Variable | `RELEASE_IMPORT_URL` | Provisioner HTTPS origin only, e.g. `https://product-provisioner.fly.dev` (no `/api/images` suffix) |
| Secret | `RELEASE_IMPORT_TOKEN` | This product's image-source `callback_token`, **not** `PP_ADMIN_TOKEN` or `PP_MASTER_KEY` |

Only enable manual `import_release` if the product uses a **callback** image
source. Registry products reject callback imports. Public GHCR images can be
pulled by Fly without private registry credentials.

After committing and pushing the workflow, tag the intended source commit:

```bash
git switch main
git tag omuse-v0.4.0
git push origin omuse-v0.4.0
```

Tag pushes run the workflow from the tagged commit, even while it lives on this
branch. Manual dispatch becomes available once the workflow is on the default
branch; choose the desired source branch and enter an `omuse-v*` version (`locius-v*` tags from before 0.4.0 are still accepted). A manual
run can enable `import_release` for a callback-backed product; it defaults to false.
Use a new version for changed source; do not move existing release tags.

Imports use `POST /api/images` with `product_key`, `version`, `image` (digest), and
`parent_version: null`. The callback retries transient failures. The
`image-manifest-<version>` Actions artifact retains the import JSON even if the
callback fails. An identical version/digest import is idempotent.

To verify a local image without any live credentials:

```bash
docker build --platform linux/amd64 -f Dockerfile.fly -t locius:local .
bash tests/docker_image_smoke.sh locius:local
```

The version shown in the app (left bar, `/sentinel/api/health`) comes from the
release tag: the workflow passes the tag as the `OMUSE_VERSION` build argument and
`omuse-v0.4.0` is shown as `v0.4.0`. The smoke test checks this when `OMUSE_VERSION`
is set, e.g. `OMUSE_VERSION=omuse-v0.4.0 bash tests/docker_image_smoke.sh locius:local`
after `docker build --build-arg OMUSE_VERSION=omuse-v0.4.0 ...`. Builds without the
argument show the version from the source.

On an Apple Silicon Mac, `--platform linux/arm64` builds a native test image;
the release workflow always builds AMD64 for Fly. The smoke test verifies all
three services, login enforcement, unprivileged startup with a root-owned volume,
Chromium PDF rendering, and settings/vault persistence after container replacement.
It deletes only its own temporary containers and volume, leaving the image.

Provisioner image settings should expose port **8080**, mount persistent storage
at **`/omuse`**, and check **`/sentinel/api/health`**. No config file is required.
Supply a generated `OMUSE_PASSWORD` and the environment variables documented above.
Port 8083 is a separate optional phone service; publishing the main UI port does
not expose it. Keep the app always-on if background schedules, event triggers,
or Telegram polling must continue while no browser is open.

### Development

```bash
cargo build --release                       # target/release/omuse
cargo test --workspace && cargo clippy --workspace -- -D warnings
./target/release/omuse --config config.toml # or --role sentinel|runtime|browser for split deployments
```

The code is a Cargo workspace under `crates/` (one crate per area: common, docs, web shell + login gate, sentinel core,
mail / apps / MCP / phone connectors, sentinel, runtime, browser broker, and the `omuse` binary). The web UI (`web/`),
the skills (`skills/`) and the browser helper scripts (`crates/omuse-browser/js/`) are compiled into the binary.
`docs/rust-port.md` records how the earlier Python version (v0.3.6, commit 562a36f) maps to the crates.

The end-to-end suites in `tests/` (Python + Playwright, kept from the Python version) run against the binary inside
Docker, with nothing on the host but Docker. Each suite gets a freshly started stack (fake LLM, fake third-party
services and the binary, configured by a generated `config.toml`):

```bash
bash tests/run_in_docker.sh                  # every suite
bash tests/run_in_docker.sh e2e_027 mcp_e2e  # only these
bash tests/docker_image_smoke.sh omuse       # the production image: config, auth, Chromium, volume
```

The test image is `Dockerfile.test` (`omuse-test`). The travel suites (`travel_*_e2e.py`) can also run with a real model:
set `OMUSE_E2E_ENV_FILE` to a file outside the repository with `OMUSE_E2E_MODEL_URL`, `OMUSE_E2E_MODEL_KEY`,
`OMUSE_E2E_PLANNER`, `OMUSE_E2E_EXECUTOR` and `OMUSE_E2E_OUT` to a directory for logs and screenshots; each suite prints a
`TIMING` line with its execution time excluding model time (measured by `tests/model_proxy.py`).

---

## 中文

OMuse 是为 [Olares](https://www.olares.com) 打造的本地优先（local-first）AI 智能体（AI Agent）。它不只是聊天：会把你的需求拆成计划，在邮件、Notion、Slack 和网页之间把事情做完。凡是发送、发布、付款、删除这类操作，都会先等你批准。

### 功能

- **对话 → 计划 → 执行**：每一步、每次工具调用都看得见。
- **邮箱（支持多个）**：Gmail、Outlook / Hotmail（OAuth 2.0 授权）、Yahoo、iCloud、QQ 邮箱、网易 163 / 126、Zoho、AOL 及任何 IMAP / SMTP 邮箱；搜索、阅读、草稿、回复、发送、一键退订；验证码和重置链接对模型自动屏蔽。
- **Notion 与 Slack**：查资料、写报告、更新任务表、读取和发送消息。
- **浏览器**：独立 Chromium，受限 API，实时画面，登录 / 验证码（CAPTCHA）可由你接管。
- **MCP 连接器**：接入任意 MCP（Model Context Protocol，模型上下文协议）服务器，工具定义锁定防篡改。
- **自动化**：持续多天推进的场景目标、事件触发（新邮件 / Slack 新消息 / Notion 变化）、定时任务。
- **Telegram 遥控**：在手机上布置任务、一键审批。
- **研究库**：交给它一个课题，它会深入研究写成 Markdown 报告；在课题对话里追问、修改，之后可刷新；所有报告都可被 Agent 搜索。
- **记忆与技能**，**中文 / English 界面**。

### 安全设计

| 层 | 作用 |
|---|---|
| **Sentinel 哨兵** | 独立的权限服务，对每个操作做 允许 / 拒绝 / 询问（ALLOW / DENY / ASK）决策。 |
| **凭据保险箱（Vault）** | 应用专用密码和令牌（token）加密保存在 Sentinel，Agent 和模型都看不到。 |
| **防提示注入（Prompt Injection）** | 邮件、网页、工具结果一律视为不可信；发现可疑内容时，所有写操作升级为人工审批。 |
| **审计记录（Audit Trail）** | 只追加、哈希链（hash chain）防篡改的日志，记录每次模型调用、工具调用、决策和审批。 |

### 安装

- **Olares 应用商店**：审核通过后搜索 *OMuse*。
- **手动安装**：`sh chart/build.sh omuse`，然后在 Olares 应用商店 ▸ *上传自定义 Chart* 中上传 `dist/omuse-<版本>.tgz`。

模型接口默认是 `https://router.<你的 Olares 名称>.olares.com/v1`，可在应用设置或 Olares 应用环境变量 `OMUSE_MODEL_URL` / `OMUSE_MODEL` 中修改；如果配置的模型不存在，OMuse 会自动选择一个可用模型。

---

License: [MIT](LICENSE) © 2026 Lucas Lu
