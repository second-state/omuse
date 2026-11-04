---
name: tax-refund-shopping
description: 游客购物退税（增值税/消费税）：日本（2026 年 11 月 1 日起出境退税）、韩国、中国大陆、台湾、新加坡 eTRS、泰国、越南、印尼；菲律宾（法律已通过、尚未实施）；马来西亚、香港、澳门没有游客退税。条件、最低消费、时限、流程与官方链接 Shopping tax refunds for visitors (VAT / GST / consumption tax) across Asia — Japan (refund at departure from 1 November 2026), Korea, mainland China, Taiwan, Singapore eTRS, Thailand, Vietnam, Indonesia; the Philippines (in law, not running yet); none in Malaysia, Hong Kong, Macau. Who can claim, the minimum, the time limits, how it works and the official links.
---
# Shopping tax refunds for visitors / 游客购物退税

The rules come from code: call **tax_refund_rules** with the countries (or cities) the user shops in. Its JSON
result is the checked table — who can claim, the minimum spend, the time limits, how it works at the shop and on the
way out, how the refund is paid, what the official pages do not confirm, and the official links; code appends them to
the reply. Never quote a threshold from memory.

## Answer
- The user asks how to get the tax back: answer from the result — eligible or not, the minimum, where and when, what
  to carry (the passport, the receipts, the goods unused), how the refund is paid — and give the official links. Say
  what the table marks as not confirmed officially.
- A country without a refund (Hong Kong, Macau: no sales tax; Malaysia: none since 2018) or not running yet (the
  Philippines): say so plainly.
- The Hong Kong **airport departure tax** refund is a different scheme: the **apdt_refund** tool (hk-apdt-refund
  skill).
- Plan it into the trip when the dates are known: the departure-day step ("tax refund — before check-in", with what
  to carry) as a reminder (schedule_create) when the user wants one.
- Expense trips: refunded tax is not a reimbursable cost — note it for `travel-expense-report`.

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
