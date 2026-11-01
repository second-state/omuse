---
name: browser-use
description: 在没有代码表单函数的网站上用浏览器填写表单（唯一允许模型在网页上输入的地方；最后的提交/付款由用户自己点）Fill a form in the browser on a site that has no code form function — the only place the model types into web pages; the final Submit / Pay is the user's. Loaded by code when a booking, checkout or form workflow reaches a site OMuse has no fixed-id form function for.
---
# Browser use on an unknown site / 未知网站的浏览器填写

Code opened this page because the site has no form function with fixed input ids. Fill it from what the run already
has — the user's request, the profile, the search result that led here — and from the Vault for secrets.

1. `browser_snapshot` the page; fill each field with `browser_type` / `browser_select` (the card number, expiry and CVC
   and any passport number only with `browser_fill_secret` from the Vault — never typed, never in the answer).
2. Leave a field blank when the run has no value for it; never invent one.
3. Stop before the final Submit / Pay / Place order / Confirm click: `browser_request_takeover`, naming what was filled
   and what is left. That click is the user's.
4. After the hand-back, `browser_snapshot` the confirmation and answer with its reference.
