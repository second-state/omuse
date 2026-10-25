---
name: payments-connectivity-asia
description: 在亚洲怎么付钱和上网：手机支付（中国支付宝/微信绑外卡、日本 PayPay、新马泰 GrabPay 等）、交通卡（Welcome Suica、八达通、T-money、EZ-Link）、现金与 ATM、汇率（market_data）、拒绝 DCC、eSIM 与漫游、银行出行报备 Paying and staying online in Asia — mobile wallets with foreign cards, transit cards, cash and ATMs, FX rates from market data, avoiding dynamic currency conversion, eSIMs and roaming, card travel notices.
---
# Payments & connectivity in Asia / 支付与上网

Mostly knowledge plus two tools: market_data for exchange rates (never from memory) and browser_read of an official
page when a rule or a fee must be current (say the date checked). No browsing for general advice.

## 1. Before the trip (checklist, tailored to destinations and length)
- **Cards**: a Visa / Mastercard that works abroad, a second card from another network; no-foreign-fee card if the user
  has one (memory_search "card", "foreign fee"); tell the issuer about the trip if its app asks for travel notices;
  know the PIN (chip-and-PIN terminals, ATMs). Amex / JCB / UnionPay acceptance varies by country.
- **Cash**: Japan, Vietnam, Indonesia, rural Thailand, small eateries and markets everywhere still take cash; airport
  ATMs beat airport exchange counters; in Japan 7-Eleven and Japan Post ATMs take foreign cards. Decline **dynamic
  currency conversion** (pay in the local currency, not the home currency) at terminals and ATMs.
- **Mobile wallets by country** (verify the current rules on the official help pages):
  - Mainland China: Alipay and WeChat Pay accept foreign Visa / Mastercard / Amex / JCB linked in the app ("Tour Pass"
    style prepaid or direct card link); register at home before departure because the SMS code is the step that fails
    most often; small payments carry no fee, larger ones a few percent; limits per transaction and per year apply.
    Without them, taxis, metro QR gates, street food and many shops are hard. Didi for taxis, Amap / Baidu for maps.
  - Japan: cards are accepted widely but failures happen at restaurants; PayPay needs a Japanese account for most
    foreigners — rely on cards, Suica-type IC cards and cash. Transit IC: Welcome Suica (28-day tourist card, no
    deposit, unused balance not refunded) or Mobile Suica / PASMO in Apple Wallet (foreign cards usually work) —
    also pays at konbini and vending machines. Regular Suica / PASMO sales have been limited since 2023; check the JR
    East page before relying on buying one.
  - Hong Kong: Octopus (tourist version, or in Apple / Google Wallet) for transit and small shops. Singapore: contactless
    bank cards tap straight through MRT / bus gates; EZ-Link optional. Korea: T-money (also WOWPASS for prepaid card +
    T-money); Kakao T for taxis. Taiwan: EasyCard / iPASS. Thailand / Malaysia / Indonesia / Vietnam / Philippines:
    Grab (ride, food, GrabPay), local QR (PromptPay, DuitNow, QRIS) often require a local bank — cards and cash.
- **Connectivity**: an eSIM bought before departure (data only, install at home, activate on landing) or a local SIM
  at the airport (passport needed); roaming packages from the home carrier as a fallback. Mainland China: many
  Western apps and sites are blocked on local networks; roaming eSIMs from abroad usually route around that — say it
  neutrally and let the user decide. Download offline maps and translation packs before landing.
- **Tipping**: not expected in Japan, Korea, China, Taiwan, Singapore; service charge is already on bills in HK / SG;
  modest tipping is common in Thailand, Indonesia, the Philippines, Vietnam for guides and hotel staff.

## 2. Exchange rates and budgets
market_data with the FX pair for the user's home currency (one call for several pairs) — quote the rate and the time;
for "how much is this in SGD" use calculate with that rate. For budgets, give per-day ranges with the assumptions.

## 3. Problems on the road
Card declined → try another network / insert instead of tap / tell the issuer via its app; ATM ate the card → the
bank's hotline from the card (never a number from a web search); wallet SMS code not arriving → try Wi-Fi off, the
home-carrier roaming, or the app's voice-call option; lost card → freeze in the app, then the issuer's emergency line
(phone_call if configured, with approval) — keep the second card separate from the first.

## 4. Answer
A short per-country checklist (pay / transit card / cash / connectivity / tipping) with the dates checked for anything
that changes, the rates from market_data, and what to set up before leaving. Say what is not verified.

## Remember what you learn (for next time)
Whatever the user tells you here is worth keeping, so they are never asked twice — but each kind has its place:
- **Profile fields** (name as in the passport, birthday, nationality, phone, emails, addresses, company, loyalty
  programme names, dietary needs, home currency, employer's expense policy, insurer's name and assistance line):
  profile_suggest(field, value) — standard fields or `custom:<label>` (e.g. `custom:passport expiry`,
  `custom:allergies`, `custom:travel insurer`); the user confirms it on the Memory page.
- **Preferences and facts** ("prefers aisle seats", "always files SGAC for the family", "company reimburses per diem
  at …"): memory_remember with its domain.
- **Secrets** — passport, ID, membership and card numbers, and photos of them: never into memory, files or the
  answer. A passport photo the user uploads may be read once with file_look to fill the form or the profile fields
  (name, birthday, nationality, expiry); then ask the user to save the number in the Vault (Memory → Profile → Vault),
  which browser_fill_secret uses from then on, and say the photo itself should not stay in the workspace.
- Uploaded documents that will be needed again (insurance policy PDF, expense policy, company template): keep them
  where they were uploaded and memory_remember the path (domain=account / work).
