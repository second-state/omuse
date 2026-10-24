//! The local tool schema (`LOCAL_TOOLS`) and the tool-name sets of `app/runtime/agent.py`.

use std::sync::LazyLock;

use serde_json::{json, Value};

use crate::prompts;

/// Tools a read-only sub-agent may use.
pub const SUBAGENT_TOOLS: [&str; 29] = [
    "gmail_search",
    "gmail_get_message",
    "gmail_get_thread",
    "gmail_read_amounts",
    "gmail_find_receipts",
    "browser_navigate",
    "browser_snapshot",
    "browser_search",
    "browser_read",
    "browser_click",
    "browser_type",
    "browser_press",
    "browser_scroll",
    "browser_back",
    "browser_wait",
    "browser_select",
    "browser_find",
    "browser_look",
    "browser_locate",
    "browser_click_at",
    "files_read",
    "file_look",
    "files_list",
    "files_search",
    "memory_search",
    "market_data",
    "stock_fundamentals",
    "calculate",
    "data_query",
];

/// local tools that change something lasting: a golden (test) run stops at them instead of running them
pub const DRY_RUN_LOCAL: [&str; 17] = [
    "memory_remember",
    "memory_forget",
    "profile_suggest",
    "schedule_create",
    "trigger_create",
    "watch_create",
    "goal_create",
    "goal_update",
    "goal_delete",
    "schedule_delete",
    "schedule_state_set",
    "notify_user",
    "files_write",
    "make_pdf",
    "make_docx",
    "make_xlsx",
    "pdf_form_fill",
];

pub const REMAKE_TOOLS: [&str; 9] = [
    "make_xlsx",
    "make_pdf",
    "make_docx",
    "make_chart",
    "make_image",
    "edit_image",
    "vary_image",
    "caption_image",
    "upscale_image",
];

/// In the last 2 steps only delivery tools remain.
pub const FINISH_TOOLS: [&str; 30] = [
    "update_plan",
    "files_write",
    "files_read",
    "file_look",
    "files_list",
    "make_pdf",
    "make_xlsx",
    "make_docx",
    "make_chart",
    "make_image",
    "edit_image",
    "vary_image",
    "caption_image",
    "upscale_image",
    "market_data",
    "stock_fundamentals",
    "calculate",
    "data_query",
    "send_file",
    "notify_user",
    "memory_remember",
    "goal_update",
    "schedule_state_set",
    "gmail_send",
    "gmail_reply",
    "gmail_create_draft",
    "slack_send_message",
    "notion_create_page",
    "notion_append",
    "calendar_create_event",
];

/// `{"type": "function", "function": {name, description, parameters}}`.
pub fn fn_(name: &str, desc: &str, props: Value, required: &[&str]) -> Value {
    json!({"type": "function", "function": {"name": name, "description": desc,
           "parameters": {"type": "object", "properties": props, "required": required}}})
}

/// The name of a tool schema entry.
pub fn tool_name(t: &Value) -> &str {
    t.get("function")
        .and_then(|f| f.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// `LOCAL_TOOLS`: the tools the runtime runs itself (Sentinel's catalog adds the connector and browser tools).
pub static LOCAL_TOOLS: LazyLock<Vec<Value>> = LazyLock::new(local_tools);

/// `LOCAL_NAMES`.
pub static LOCAL_NAMES: LazyLock<Vec<String>> = LazyLock::new(|| {
    LOCAL_TOOLS
        .iter()
        .map(|t| tool_name(t).to_string())
        .collect()
});

pub fn is_local(name: &str) -> bool {
    LOCAL_NAMES.iter().any(|n| n == name)
}

fn local_tools() -> Vec<Value> {
    let s = json!({"type": "string"});
    let app_id = prompts::app_id();
    vec![
        fn_("update_plan", "更新任务计划 Update the task plan: mark steps done/failed/running or replace the steps when re-planning.",
            json!({"steps": {"type": "array", "items": {"type": "object", "properties": {
                "id": s, "description": s, "status": {"type": "string", "enum": ["pending", "running", "done", "failed", "skipped"]}}}},
             "objective": s, "note": s}), &["steps"]),
        fn_("files_list", &format!("列出工作区文件 List files in the workspace (Olares Files → Data/{}/workspace).", app_id), json!({"path": s}), &[]),
        fn_("files_read", "读取工作区文件的文字（文本/Markdown/CSV/JSON/PDF/Word/Excel/PowerPoint）Read the text of a workspace file \
(text, Markdown, CSV, JSON, PDF, Word .docx, Excel .xlsx, PowerPoint .pptx).", json!({"path": s, "max_chars": {"type": "integer"}}), &["path"]),
        fn_("file_look", "用视觉模型看工作区里的图片、视频（抽取画面）、扫描版 PDF，或听音频/视频里的讲话，回答你的问题。\
LOOK at a workspace image, a video (still frames + the speech, if a speech model is available), a scanned PDF, or \
LISTEN to an audio file, and answer your question about it — e.g. files the user attached or media you saved.",
            json!({"path": s, "question": {"type": "string", "description": "what you want to know about the file"}}), &["path", "question"]),
        fn_("files_write", "写入工作区文件（报告、笔记、数据）；append=true 在末尾追加。返回字数。Write a text file in the workspace (creates \
folders); append=true adds to the end. Returns the word / character count.",
            json!({"path": s, "content": s, "append": {"type": "boolean"}}), &["path", "content"]),
        fn_("files_search", "在工作区文件中搜索文字 Search text inside workspace files.", json!({"query": s}), &["query"]),
        fn_("send_file", "把工作区里的文件发到对话里（报告、PDF、表格、照片、视频等）：图片直接显示，视频/音频可以直接播放，其他文件可下载；\
多张照片用 paths 一次发出。如果对话来自 Telegram，也会发到 Telegram。用户在对话里要照片或文件时就用它发，不要改用邮件，\
也不要让用户自己去 Files 里找。\
 Send workspace files into this chat — images show inline, videos/audio play inline, anything else is a download \
(and they go to Telegram when the chat came from there). Pass several photos at once with `paths`. When the user \
asks in the chat for photos or files, send them here — not by email unless they ask for email.",
            json!({"path": s, "paths": {"type": "array", "items": s, "description": "several files at once (max 20)"},
             "note": {"type": "string", "description": "一句说明 one-line caption"}}), &[]),
        fn_("make_pdf", "在本机把工作区文件（.md / .txt / .html）或一段 Markdown 生成 PDF（支持中文、表格、图片，A4，带页码）。\
所有文件都在本机处理——绝对不要把用户的文件上传到在线转换网站。生成后如果用户要文件，用 send_file 发给他。\
 Make a PDF locally from a workspace file (.md/.txt/.html) or Markdown text. Never upload the user's documents \
to online converters. Then use send_file if the user wants the file.",
            json!({"source": {"type": "string", "description": "工作区里的源文件 workspace path of the source (.md/.txt/.html)"},
             "markdown": {"type": "string", "description": "或者直接给 Markdown 内容 or Markdown text instead of a file"},
             "output": {"type": "string", "description": "输出路径，默认与源文件同名 .pdf output path (default: next to source)"},
             "title": s}), &[]),
        fn_("make_docx", "在本机生成 Word 文档（.docx），不要用任何在线转换网站。内容用 Markdown 写：# 标题、## 小标题、段落、**粗体**、\
列表、表格、> 引用，以及 ![说明](charts/xxx.png) 插入工作区里的图片（例如 make_chart send=false 生成的图）。\
 Create a Word document (.docx) locally — never use online converters. Write the content as Markdown (headings, \
paragraphs, bold, lists, tables, quotes, ![caption](charts/x.png) images from the workspace) or pass a workspace \
.md file as source; then send_file it.",
            json!({"markdown": {"type": "string", "description": "文档内容（Markdown）document content in Markdown"},
             "source": {"type": "string", "description": "或者：工作区里的 .md 文件 or a workspace .md file"},
             "output": {"type": "string", "description": "输出路径，以 .docx 结尾 output path ending in .docx, e.g. reports/summary.docx"},
             "title": {"type": "string", "description": "可选：文档标题 optional title"}}),
            &["output"]),
        fn_("make_xlsx", "在本机生成 Excel 表格（.xlsx）：表头加粗、首行冻结、可筛选、数字按数字存。可以有多个工作表。\
数据用 sheets 给出，或者用 source 指定工作区里的 CSV / 含 Markdown 表格的文件。生成后用 send_file 发给用户。\
 Make an Excel workbook locally. Give sheets=[{name, columns:[...], rows:[[...], ...]}] or source (a workspace .csv, \
or a .md file containing a Markdown table). Then use send_file if the user wants the file.",
            json!({"output": {"type": "string", "description": "工作区里的输出路径 output path ending in .xlsx"},
             "sheets": {"type": "array", "items": {"type": "object", "properties": {
                 "name": s, "columns": {"type": "array", "items": s},
                 "rows": {"type": "array", "items": {"type": "array", "items": {}}}}}},
             "source": {"type": "string", "description": "或者：工作区里的 .csv / .md 文件 or a workspace .csv/.md file"},
             "formulas": {"type": "boolean", "description": "保留以 = 开头的公式（如 =SUM(B2:B9)）keep simple formulas; default false"}}),
            &["output"]),
        fn_("calculate", "精确计算（不要心算）：贷款月供和还款明细、利息、复利、增长率、汇率换算、AA 分摊、百分比、合计和平均。\
表达式支持 + - * / ** % 和 round、min、max、sum、mean、median、sqrt、log；金融函数：pmt(月利率, 期数, 本金)、\
loan(本金, 年利率%, 年数, 明细行数) 返回月供+总利息+还款明细、invest(每期投入, 年化收益%, 年数, 初始本金, 每年期数) 返回每年末余额/累计投入/收益（定投、储蓄；每年存一次用 per_year=1）、\
fv(每期利率, 期数, 每期存入, 现值)、cagr(起始, 结束, 年数)、npv(折现率%, [现金流…]) 净现值、irr([现金流…]) 内部收益率%、\
apr(本金, 每期还款, 期数) 分期/贷款的真实年利率、payback(成本, 每期节省) 回本期数。年利率一律写百分数（4% 写 4）。\
 Exact arithmetic — never compute figures the user relies on in your head. Operators + - * / ** %, functions round \
min max sum mean median sqrt log, finance: pmt(rate, nper, pv), loan(principal, annual_rate_pct, years, rows) → \
payment, totals and amortization rows, invest(deposit, annual_rate_pct, years, initial, per_year=12) → year-by-year balance, money \
put in and gain (regular saving / investing; per_year=1 for one deposit a year), fv(rate_per_period, nper, pmt, pv), cagr(start, end, years), \
npv(rate_pct, [flow0_now, flow1, …]), irr([flows]) → % per period, apr(principal, payment, nper) → true APR and effective \
rate of an instalment plan, payback(cost, saving_per_period). Annual rates are percentages: 4% is 4, not 0.04.",
            json!({"expressions": {"type": "array", "items": s, "description": "要算的表达式 expressions, e.g. [\"loan(3000000, 3.5, 25, 12)\", \"283.8/4\"]"},
             "variables": {"type": "object", "description": "可选：变量 optional named values (may be expressions), e.g. {\"r\": \"0.035/12\"}"}}),
            &["expressions"]),
        fn_("data_query", "精确分析表格数据（CSV、Excel、日志），不要自己数行或心算：计数、求和、平均、中位数、标准差、百分位（p95）、分组汇总、\
占比、透视表、分段（年龄段、分数段）、筛选、排序、排名、环比、z 分数（找异常值）、线性趋势预测。先不带查询调用一次看列名和概况，\
再一次传多个 queries。结果可 save 成 .csv/.xlsx（make_xlsx 可直接用作 source）。日志等文本文件传 pattern（带命名分组的正则）。\
 Exact numbers from tables (CSV, Excel, text logs) — never count rows or add up a column yourself. Call once with only \
path to see the columns, then pass several queries in one call. Query keys, applied in this order: \
derive [{as, expr: arithmetic over columns written as they are named, e.g. \"Amount (SGD) - Budget (SGD)\"} | {as, from, bins:[edges], labels} | \
{as, from, map:{old:new}} | {as, from, date_part: year|quarter|month|week|date|weekday|hour} | {as, from, slice:[0,7]} | \
{as, zscore|pct_change|rank|cumsum: col, by:[cols]}], where [{col, op (== != > >= < <= contains in between regex \
empty), value — a value naming a column compares the two columns}], where_any (OR), then ONE of group_by:[cols] + agg:[{col, fn: count|count_distinct|\
sum|mean|median|min|max|std|var|range|p90|p95|share|count_share|mode|list, as}] / pivot:{rows, cols, value, fn, totals} / \
trend:{y, x?, by?, ahead} (least-squares line + forecast); then having (where on the grouped result), \
sort [{col, desc}], select, totals:true, limit, \
save:\"analysis/x.csv\".",
            json!({"path": {"type": "string", "description": "工作区里的 .csv / .xlsx / .txt 文件 workspace file (attachments too)"},
             "sheet": {"type": "string", "description": "可选：工作表名 optional sheet name"},
             "pattern": {"type": "string", "description": "可选：把文本/日志每行解析成一行数据的正则，用命名分组 optional regex with named groups, \
e.g. ^(?P<time>\\S+) (?P<ip>\\S+) \"(?P<method>\\S+) (?P<path>[^\"]+)\" (?P<status>\\d{3}) (?P<ms>\\d+)ms"},
             "queries": {"type": "array", "items": {"type": "object"}, "description": "查询列表（每个是上面说明的对象）list of query objects; omit to describe the table"}}),
            &["path"]),
        fn_("stock_fundamentals", "查公司估值和财报（Yahoo Finance，不用开浏览器）：股价、市值、市盈率 P/E（TTM 和预期）、市净率、EPS、股息率、\
最近 4 个季度和 4 个年度的营收、毛利（毛利率）、经营利润、净利润（净利率）、摊薄 EPS。估值对比、财报要点一律先用它。\
代码同 market_data（TSLA、1211.HK、300750.SZ…）。分析师观点和新闻仍需查网页。\
 Valuation and reported financials (no browser): price, market cap, P/E (TTM, forward), P/B, EPS, dividend yield and the \
last 4 quarters / 4 years of revenue, gross profit and margin, operating income, net income and margin, diluted EPS.",
            json!({"symbols": {"type": "array", "items": s, "description": "1–6 个代码 tickers, e.g. [\"TSLA\", \"1211.HK\"]"}}),
            &["symbols"]),
        fn_("market_data", "查股票/指数/汇率/黄金/加密货币的行情和历史价格（Yahoo Finance，不用开浏览器）：最新价、区间涨跌、区间最高/最低（带日期）、\
52 周区间和收盘价序列。股价、走势、涨跌幅一律先用它，比翻网页快且准。代码示例：AAPL、NVDA、0700.HK（腾讯）、0005.HK（汇丰）、\
9988.HK、600519.SS、300750.SZ、^GSPC（标普500）、^IXIC（纳指）、^DJI、^HSI（恒指）、^N225（日经）、HKD=X（美元兑港币）、\
CNY=X（美元兑人民币）、GC=F（黄金）、BTC-USD。也可以给英文公司名。估值（市盈率等）和财报用 stock_fundamentals。\
 Quotes and price history for stocks, indices, FX, gold and crypto (Yahoo Finance; no browser): last price, change over \
the range, high/low with dates, 52-week range and a close-price series. Use it first for any price / trend question.",
            json!({"symbols": {"type": "array", "items": s, "description": "1–8 个代码 tickers, e.g. [\"0700.HK\", \"^HSI\"]"},
             "range": {"type": "string", "enum": ["5d", "1mo", "3mo", "6mo", "ytd", "1y", "2y", "3y", "5y", "10y", "max"],
                       "description": "时间范围 history range (default 1y)"}}),
            &["symbols"]),
        fn_("make_chart", "在本机画图表/示意图并直接显示在对话里（PNG）：走势图 line、柱状/排行 bar（horizontal=true 横向排行）、\
饼图 pie、甘特图 gantt、流程图/架构图 flow。用户要「画图、图表、走势图、对比图、饼图、流程图、架构图、甘特图」时就用它——\
不要写代码、不要打开在线画图或代码运行网站、不要用 ASCII 字符画。数字必须来自你在本任务里查到的数据或用户给的数据，\
source 写明来源；不要编造。生成后图片自动发到对话（send=false 则只保存，可在 make_pdf 的 Markdown 里用 ![标题](路径) 插入）。\
 Draw a chart or diagram locally and show it in the chat as a PNG: line (trends; several series ok; highest/lowest \
marked), bar (grouped; horizontal=true for rankings; negative values ok), pie, gantt, flow (flowchart / architecture). \
Use it whenever the user asks for a chart, graph, plot or diagram — never write code, never use online chart or \
code-runner sites, no ASCII art. Only plot numbers you fetched in this task or the user gave, and name the source.",
            json!({"type": {"type": "string", "enum": ["line", "bar", "pie", "gantt", "flow"]},
             "title": s, "subtitle": s, "source": {"type": "string", "description": "数据来源（网站名/网址）data source"},
             "labels": {"type": "array", "items": s, "description": "line/bar 的 X 轴（日期或类别）；pie 的扇区名 x-axis labels / slices"},
             "series": {"type": "array", "items": {"type": "object", "properties": {"name": s, "values": {"type": "array", "items": {}}}},
                        "description": "line/bar：一条或多条数据线，values 与 labels 一一对应 one or more series, values match labels"},
             "values": {"type": "array", "items": {}, "description": "pie（或只有一条线时）的数值 values for pie / a single series"},
             "unit": {"type": "string", "description": "单位，如 %、$、港元 unit shown on values"},
             "horizontal": {"type": "boolean", "description": "bar 横向（排行榜）horizontal bars for rankings"},
             "sort": {"type": "string", "enum": ["desc", "asc"], "description": "bar 排序 sort a single-series bar chart"},
             "tasks": {"type": "array", "items": {"type": "object", "properties": {"name": s, "start": s, "end": s, "days": {"type": "number"},
                                                                                    "weeks": {"type": "number"}}},
                       "description": "gantt：[{name, start YYYY-MM-DD, end 或 days/weeks}]；没有 start 的接在上一项后面"},
             "nodes": {"type": "array", "items": {"type": "object", "properties": {"id": s, "label": s}}, "description": "flow 的方框"},
             "edges": {"type": "array", "items": {"type": "object", "properties": {"from": s, "to": s, "label": s}}, "description": "flow 的箭头"},
             "steps": {"type": "array", "items": s, "description": "flow 的简单写法：按顺序的步骤 a simple linear flow"},
             "direction": {"type": "string", "enum": ["LR", "TB"], "description": "flow 方向：LR 横向 / TB 竖向"},
             "symbols": {"type": "array", "items": s, "description": "line 走势图的快捷方式：直接给股票/指数/汇率代码，自动取数据画图（不用自己抄数字）\
 shortcut for a price chart: tickers to fetch and plot (with range)"},
             "range": {"type": "string", "description": "配合 symbols 的时间范围 range for symbols: 1mo/3mo/6mo/ytd/1y/2y/3y/5y/10y"},
             "mode": {"type": "string", "enum": ["price", "pct"], "description": "配合 symbols：price 价格；pct 涨跌幅%（不同股票/币种对比时用）"},
             "output": {"type": "string", "description": "保存路径，默认 charts/<标题>.png output .png path"},
             "send": {"type": "boolean", "description": "是否发到对话里（默认 true）show it in the chat (default true)"}}),
            &["type", "title"]),
        fn_("make_image", "用 AI 根据文字描述生成一张图片（像 Midjourney / DALL·E）并直接显示在对话里：插画、海报、头像、产品图、\
场景、概念图、logo 草稿、壁纸等。用户说「画一张…、生成一张…图、做个海报/头像/壁纸、来张…的图」时就用它——\
不要去打开在线作图网站，也不要用 make_chart（那是数据图表）。prompt 用英文写得具体：主体、场景、风格、光线、构图、\
颜色；用户给的中文描述你翻译并补全细节。aspect 按用途选：海报/手机壁纸 portrait，横幅/桌面壁纸 landscape，头像/logo square。\
生成后图片自动发到对话；用户要改时（换风格、换颜色、加东西）用新的 prompt 再调一次。\
 Generate an image from a text description with an AI image model (like Midjourney / DALL·E) and show it in the \
chat: illustrations, posters, avatars, product shots, scenes, concept art, logo drafts, wallpapers. Use it whenever \
the user asks to draw / generate / make a picture, poster, avatar or wallpaper — never open an online image site, \
and not make_chart (that is for data charts). Write the prompt in English and concretely (subject, setting, \
style, lighting, composition, colours); translate and enrich a Chinese request. Pick aspect by use. The image is \
sent to the chat automatically; to change it, call again with a new prompt.",
            json!({"prompt": {"type": "string", "description": "英文、具体的画面描述 a concrete English description of the picture"},
             "aspect": {"type": "string", "enum": ["square", "landscape", "portrait"],
                        "description": "画幅：square 方形(头像/logo)，landscape 横向(横幅/桌面)，portrait 竖向(海报/手机)"},
             "size": {"type": "string", "description": "或明确尺寸 or an explicit size like 1024x1024 / 1536x1024 / 1024x1536"},
             "n": {"type": "integer", "description": "张数 1-4（默认 1）how many variations"},
             "quality": {"type": "string", "enum": ["low", "medium", "high"], "description": "质量（支持时）quality when the model supports it"},
             "style": {"type": "string", "description": "风格（中英皆可，会展开成具体提示词）：写实 photorealistic / 电影感 cinematic / 动漫 anime / 水彩 watercolor / 油画 oil painting / 3D render / 扁平矢量 flat vector / 像素 pixel art / 素描 sketch / 水墨 ink wash / 线稿 line art / 等距 isometric / 极简 minimalist / logo / 产品图 product"},
             "negative_prompt": {"type": "string", "description": "不想出现的东西（支持的模型才生效）things to avoid, e.g. 'text, watermark, blurry, extra fingers'"},
             "background": {"type": "string", "enum": ["transparent", "opaque"], "description": "透明背景（logo/贴纸/产品抠图；支持的模型才生效）transparent background when the model supports it"},
             "output_format": {"type": "string", "enum": ["png", "jpeg", "webp"], "description": "输出格式，默认 png"},
             "title": {"type": "string", "description": "文件名用的短标题 short title for the file name"},
             "output": {"type": "string", "description": "保存路径，默认 images/<标题>.png output .png path"},
             "send": {"type": "boolean", "description": "是否发到对话里（默认 true）show it in the chat (default true)"}}),
            &["prompt"]),
        fn_("edit_image", "用 AI 修改一张已有图片（像 Midjourney 编辑器 / ChatGPT 选区编辑）：换颜色、加/去掉东西、换背景、改风格、\
按参考图改。path 是工作区里的图（刚生成的、用户附件、browser_save_media 存的）；不给 path 就改本任务最近那张。\
mask 可选：一张 PNG，透明的地方就是要重画的区域（局部重绘 inpainting）；不给 mask 就整张按 prompt 改。\
references 可选：几张参考图（风格/人物/物体一致，对标 Midjourney --sref / --oref）。\
用户说「把这张图…改成/换成/加上/去掉」「按这张的风格」时用它；不要重新 make_image 从头生成。\
 Edit an existing image with the AI model: recolour, add/remove things, swap background, restyle, or follow \
reference images. `path` = a workspace image (just generated, a user attachment, or saved from a page); omit it \
to edit this task's latest image. Optional `mask` PNG: transparent pixels = the region to repaint (inpainting); \
without a mask the whole image is edited by prompt. Optional `references`: images to keep style/character/object \
consistent (like --sref / --oref). Use it when the user wants an existing picture changed — don't regenerate.",
            json!({"path": {"type": "string", "description": "要改的图片 workspace path of the image to edit (default: latest image of this task)"},
             "prompt": {"type": "string", "description": "英文、具体地说明改什么 what to change, concretely, in English"},
             "mask": {"type": "string", "description": "可选：遮罩 PNG 的工作区路径，透明区域=重画区域 optional mask PNG (transparent = repaint here)"},
             "references": {"type": "array", "items": s, "description": "可选：参考图路径 optional reference images (style / character / object)"},
             "aspect": {"type": "string", "enum": ["square", "landscape", "portrait"]},
             "size": {"type": "string", "description": "或明确尺寸 or an explicit size"},
             "n": {"type": "integer", "description": "张数 1-4"},
             "title": {"type": "string", "description": "文件名用的短标题"},
             "send": {"type": "boolean", "description": "是否发到对话里（默认 true）"}}),
            &["prompt"]),
        fn_("vary_image", "基于一张图再生成几张「差不多但不一样」的变体（像 Midjourney 的 Vary / 再来几张）：主体、构图、风格不变，细节有变化。\
不给 path 就用本任务最近那张。用户说「再来几张类似的」「换几个版本」「微调一下」时用它。\
 Make close variations of an image (like Midjourney Vary): same subject, composition and style with small \
differences. Omit `path` for this task's latest image. Use when the user asks for more versions / similar ones.",
            json!({"path": {"type": "string", "description": "原图 workspace path (default: latest image of this task)"},
             "prompt": {"type": "string", "description": "可选：变化方向 optional direction, e.g. 'warmer colours', 'from the side'"},
             "n": {"type": "integer", "description": "张数 1-4（默认 2）"},
             "title": {"type": "string"}, "send": {"type": "boolean"}}), &[]),
        fn_("caption_image", "把一段**准确的文字**压到图片上（海报标题、口号、价格、logo 字样、字幕）。AI 图像模型写字经常拼错或乱码，\
这个工具由本机排版、字对字保证正确，支持中文。不给 path 就用本任务最近那张图。用户要「海报上写…」「加标题/文字/口号」\
「logo 下面写公司名」时：先 make_image 生成不带文字的底图，再用它加字。\
 Put EXACT text on an image (poster title, slogan, price, wordmark, caption). Image models misspell or garble \
text; this lays it out locally, letter-perfect, Chinese included. Omit `path` for this task's latest image. For \
'write … on the poster', generate the picture without text first (make_image), then add the words with this.",
            json!({"path": {"type": "string", "description": "底图 workspace image (default: latest image of this task)"},
             "text": {"type": "string", "description": "要写的文字（可多行）the exact text, line breaks allowed"},
             "position": {"type": "string", "enum": ["top", "bottom", "center", "top-left", "top-right", "bottom-left", "bottom-right"],
                          "description": "位置，默认 bottom"},
             "size": {"type": "integer", "description": "字号 px（默认按图片大小自动）font size in px (auto by default)"},
             "color": {"type": "string", "description": "文字颜色，如 #ffffff / #111 / gold（默认白）"},
             "band": {"type": "boolean", "description": "文字下垫半透明色带，复杂背景上更清楚 a translucent band behind the text"},
             "weight": {"type": "string", "enum": ["400", "500", "600", "700", "800", "900"], "description": "字重，默认 700"},
             "title": {"type": "string"}, "send": {"type": "boolean"}}),
            &["text"]),
        fn_("upscale_image", "把图片放大 2 倍或 4 倍（本机像素放大 + 锐化，不联网）。用户要「放大」「高清一点」「做成大图/打印尺寸」时用。\
注意：这是像素放大，不会凭空生成新细节（真正的 AI 超分需要专门模型，目前没有）。不给 path 就用本任务最近那张。\
 Enlarge an image 2x or 4x locally (resampling + sharpening, offline). Use for 'make it bigger / higher-res / print \
size'. Note: pixel upscaling, it does not invent new detail (true AI super-resolution needs a dedicated model, \
not available). Omit `path` for this task's latest image.",
            json!({"path": {"type": "string"}, "factor": {"type": "integer", "enum": [2, 4], "description": "放大倍数，默认 2"},
             "title": {"type": "string"}, "send": {"type": "boolean"}}), &[]),
        fn_("present_choices", "把几个选项做成卡片给用户挑（餐厅、商品、航班、方案等）。kind=comparison 时，每个选项的 label 和 details \
必须是你在本任务里读过的网页/邮件的原文摘录（照抄原文，不要翻译或改写），source_url 是读过的那个页面（或其中的链接）；\
系统会逐条核对，找不到原文就拒绝显示。你的翻译、评价写在 note 里（不核对）。kind=clarify 用于简单的澄清选项（不核对）。\
 Show options as cards the user can pick from. For kind=comparison every option's label and each detail MUST be an \
exact excerpt copied from a page or email you read in this task (same language, no paraphrase), with source_url = \
that page (or a link on it); OMuse checks each one against what you actually read and refuses unverifiable cards. \
Put translations, opinions and your recommendation in note (not checked). kind=clarify: plain choices, not checked. \
After calling it, finish with a short answer — the user's pick arrives as their next message.",
            json!({"question": s, "kind": {"type": "string", "enum": ["comparison", "clarify"]},
             "options": {"type": "array", "items": {"type": "object", "properties": {
                 "label": s, "details": {"type": "array", "items": s}, "source_url": s, "note": s}}}}),
            &["question", "options"]),
        fn_("pdf_form_fields", "列出 PDF 表格里可填写的字段（名称、类型、当前值、选项、所在页）。在本机处理，不上传。\
 List the fillable fields of a PDF form in the workspace (name, type, current value, options, page). Local only.",
            json!({"path": s}), &["path"]),
        fn_("pdf_form_fill", "在本机填写 PDF 表格，生成一份填好的副本（原件不动）。values = {字段名: 值}；勾选框用 true/false，单选/下拉用列出的选项。\
只填用户告诉你的或记忆里明确的信息，不知道的先问用户，绝不编造；签名栏留给用户自己签。填好后用 send_file 给用户检查，\
用户确认后才可以用 gmail_reply 带 attachments 发出去。\
 Fill a PDF form locally and save a filled COPY (the original is untouched). values = {field name: value}; \
checkboxes true/false, radio/dropdown one of the listed options. Only use facts the user gave you (ask for \
missing ones, never invent); never fill signatures. Then send_file it for the user to check before any email.",
            json!({"path": s, "values": {"type": "object"}, "output": {"type": "string", "description": "default: <name>-filled.pdf next to it"}}),
            &["path", "values"]),
        fn_("memory_search", "搜索记忆 Search memory about the user (long-term facts, recent details and past tasks).", json!({"query": s}), &["query"]),
        fn_("memory_remember", "记住用户明确要求记住的事实 Save a durable fact the user explicitly asked to remember, or a site's form habit you just learned at checkout \
(e.g. \"decathlon.sg accepts the phone number as 8 digits without +65\"). Not for profile \
fields (name, phone, email, address… → profile_suggest) and never for ID / membership / card numbers or passwords \
(those live in the Sentinel vault, which the user manages). domain: person | preference | place | account | site \
(how a website works for the user) | rule (a standing instruction) | work. Something you learned yourself (not \
said by the user) is kept as 'to confirm' until the user OKs it on the Memory page — it is still used meanwhile.",
            json!({"fact": s, "category": s, "entity": s, "domain": s}), &["fact"]),
        fn_("profile_get", "读取用户档案 Read the user's profile (name as on passport, phone, emails, addresses, company, title, \
birthday, nationality…). Call it ONLY when filling in a form or writing an email/message that needs these details; \
use just the fields you need.", json!({"fields": {"type": "array", "items": s, "description": "optional: only these fields"}}), &[]),
        fn_("profile_suggest", "建议修改档案 Propose a change to a profile field when the user told you a new value (e.g. a new phone \
number). It is NOT applied until the user confirms it on the Memory page. field = one of: name_zh, name_en, \
preferred_name, phone, email_personal, email_work, address_home, address_work, company, job_title, birthday, \
nationality, or custom:<label>.", json!({"field": s, "value": s, "reason": s}), &["field", "value"]),
        fn_("vault_list", "列出保险箱条目 List the items in the user's Sentinel vault (ID documents, membership numbers, payment \
cards) — labels and the last 4 characters only, never the values. Use with browser_fill_secret to fill one into a \
web form; each fill needs the user's approval.", json!({}), &[]),
        fn_("memory_forget", "删除一条记忆 Forget a memory by id (from memory_search).", json!({"id": s}), &["id"]),
        fn_("orders_list", "交易账本：OMuse 替用户下过的订单（商家、订单号、金额、卡尾号、状态、谁批准的）。用户说「取消/退货/查一下那个订单」时先查这里，\
用账本里那一单的商家和订单号去操作，不要凭邮件或别的网站猜。\
 The ledger of orders OMuse placed for the user (merchant, order number, amount, card, status, who approved). \
For cancel / return / track requests look the order up here first and act on exactly that merchant and number.",
            json!({"query": {"type": "string", "description": "optional: merchant or order number"}}), &[]),
        fn_("schedule_create", "创建定时/周期任务 Create a recurring background task. kind=cron (spec like '0 8 * * *') or \
interval (spec = minutes). goal = full standalone instruction for each run.",
            json!({"name": s, "goal": s, "kind": {"type": "string", "enum": ["cron", "interval"]}, "spec": s}), &["name", "goal", "kind", "spec"]),
        fn_("trigger_create", "创建事件触发器：当某事发生时自动运行一个任务 Create an event trigger (\"when X happens, do Y\"). \
source: gmail.new_email (params: query e.g. 'from:boss@acme.com', account) | slack.new_message (params: channel, \
keyword, mentions_only) | notion.db_changed (params: database_id). goal = full standalone instruction for each run; \
the new items are handed to that run as data. every = poll minutes (default 3).",
            json!({"name": s, "goal": s, "source": {"type": "string", "enum": ["gmail.new_email", "slack.new_message", "notion.db_changed"]},
             "params": {"type": "object"}, "every": {"type": "number"}}), &["name", "goal", "source"]),
        fn_("watch_create", "监控一个网页：内容变化、出现某段文字（如“有货 In stock”），或价格低于阈值时提醒用户。只在状态“新变成满足”时提醒一次，\
不会重复打扰；检查失败会自动拉长间隔，连续失败会自动停用并告知。默认只发通知（不运行 Agent、不耗模型）；给了 then 才会在触发时运行那个任务。\
 Watch a public web page and alert the user when it changes, when some text appears, or when a price drops below a \
threshold. Alerts fire once per new change (deduplicated); failures back off. By default it only notifies (no agent \
run); give `then` to run a task with the change as data instead.",
            json!({"name": s, "url": s, "mode": {"type": "string", "enum": ["change", "text", "price_below"]},
             "text": {"type": "string", "description": "mode=text: the words to wait for, e.g. 'In stock'"},
             "threshold": {"type": "number", "description": "mode=price_below: alert when a price on the page is below this"},
             "keyword": {"type": "string", "description": "optional: only prices/text near this word (e.g. the product name)"},
             "current_price": {"type": "number", "description": "mode=price_below: the item's price as you saw it on the page now \
(e.g. 31.43). The watch reads the page itself and refuses to start if it reads a different price."},
             "every_minutes": {"type": "number", "description": "check interval, default 60, minimum 15"},
             "then": {"type": "string", "description": "optional standalone instruction to run when it fires"}}),
            &["name", "url", "mode"]),
        fn_("goal_create", "创建长期目标（场景目标）：OMuse 会定期检查并持续推进，直到达成 Create a long-running goal that OMuse keeps \
working on until achieved (e.g. 'get John to confirm the contract by Friday', 'keep inbox under 20 unread'). \
check_kind: interval (check_spec = minutes, >= 5) | cron (e.g. '0 9 * * *') | event (check_spec = JSON like trigger_create: \
{\"source\": ..., \"params\": {...}}). deadline: optional 'YYYY-MM-DD' or 'YYYY-MM-DD HH:MM'.",
            json!({"title": s, "objective": s, "success_criteria": s, "check_kind": {"type": "string", "enum": ["interval", "cron", "event"]},
             "check_spec": s, "deadline": s}), &["title", "objective", "check_kind", "check_spec"]),
        fn_("goal_list", "列出长期目标 List goals and their status.", json!({}), &[]),
        fn_("goal_update", "记录本次目标检查的结果（目标检查任务结束前必须调用）Record the outcome of this goal check (MUST be called \
at the end of every goal run). status: active (still working) | blocked (need the user's help/decision) | \
achieved (success criteria met) | failed (cannot be achieved). progress = what you found/did, concise.",
            json!({"status": {"type": "string", "enum": ["active", "blocked", "achieved", "failed"]}, "progress": s}), &["status", "progress"]),
        fn_("schedule_list", "列出定时任务 List schedules.", json!({}), &[]),
        fn_("schedule_delete", "删除定时任务 Delete a schedule by id.", json!({"id": s}), &["id"]),
        fn_("goal_delete", "删除长期目标（连同它的定期检查）Delete a long-running goal by id (goal_list shows ids), together with \
its check schedule. Only when the user asks to remove that goal.", json!({"id": s}), &["id"]),
        fn_("schedule_state_get", "读取本定时任务上次保存的状态 Read state saved by previous runs of this schedule.", json!({}), &[]),
        fn_("schedule_state_set", "保存本定时任务的状态（用于下次对比变化）Save state for the next run of this schedule.",
            json!({"key": s, "value": s}), &["key", "value"]),
        fn_("notify_user", "给用户发通知（应用内 + Telegram 如已配置）Notify the user (in-app + Telegram if configured).",
            json!({"title": s, "message": s}), &["title", "message"]),
        fn_("delegate", "派出子 Agent 独立完成一个调研类子任务（只读工具），返回报告 Spawn a read-only sub-agent for a focused sub-task.",
            json!({"role": s, "task": s}), &["role", "task"]),
        fn_("load_skill", "加载技能说明 Load a skill's detailed instructions by name.", json!({"name": s}), &["name"]),
        fn_("library_list", "列出研究库里的课题 List the research subjects in the user's library (title, status, report file).", json!({}), &[]),
        fn_("library_search", "在研究库的报告里搜索 Search the reports in the research library (passages with their subject).", json!({"query": s}), &["query"]),
        fn_("library_get", "读取研究库里一份完整报告 Read a research subject's whole report by title or slug.", json!({"subject": s}), &["subject"]),
    ]
}
