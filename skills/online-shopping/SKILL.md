---
name: online-shopping
description: 网购：用户给了商店链接时用 shop_order（比较 / 加入购物车 / 购买，条件作为参数），代码排名、加购、填写结账页，下单由用户点；没有链接时按下面的步骤搜索 Shop online — with a shop link the user gave, call shop_order (compare, cart or buy, the criteria as JSON arguments): code ranks the listings, adds to the cart and fills the checkout, Place order is the user's; without a link, search as below.
form: on
---
# Online shopping / 网购（搜索、挑选、加入购物车）

## A shop link the user gave → shop_order
Call `shop_order` once with the user's words as arguments — never open, read or click the shop's pages yourself:
- `url` = the link; `action` = `compare` (show / find / which is best), `cart` (add it to my cart) or `buy` (order it,
  buy it now); `by` = `rating` (best-rated), `price` (cheapest) or `ratings` (most reviewed); `max_price`,
  `min_price`, `min_stars`, `min_ratings`, `count` (how many to show), `title_has` (words every chosen name has: the
  product word and the exact device model, e.g. ["case", "iPhone 17 Pro Max"]), `title_not`.
- `let_user_choose: true` when the user wants to pick ("show me options", "I'll pick one"); `pick` = the listing the
  user chose on a card ("I choose: …"); `colour`, `quantity`, `card` (the Vault card named), `contact` (name, phone,
  delivery address the user gave; the profile comes first).
- The result: `shown` (the listings ranked, the pick — or `choose: true`: the cards are on screen, the user picks),
  `in_cart` (in the cart, not checked out), `booked` (the order number), `not_confirmed`, `nothing_found`, `error`,
  `browser_use` (a shop code has no form for: its page is open and the browser-use skill is loaded — continue there).
  Every fact in `must_say` goes into the reply. `needs_input` / `invalid`: ask the user / fix the arguments.

## No link: searching shops yourself
1. **Search with a direct URL** (fast, no typing):
   Amazon: `https://www.amazon.<sg|com|co.jp…>/s?k=<words+joined+by+plus>` · Lazada: `https://www.lazada.sg/catalog/?q=<words>` ·
   Shopee: `https://shopee.sg/search?keyword=<words>` · other shops: their own search-URL template when you know it, else web_search("<words> site:<shop domain>").
   FairPrice: `https://www.fairprice.com.sg/search?query=<words>` · Lazada / Shopee may show a robot check → takeover.
   **A shopping list of several items** (2026-10-04 V3-03: 12 items were searched three times over and nothing was written
   down): read 3–4 search URLs per browser_read call, and as soon as you have an item's best match and price, write it down
   with update_plan(note=…) or files_write — older results are compressed out of your context. Never search an item again
   that is already in your notes; when the list is done, answer from the notes.
   **Match the list exactly** (2026-10-05 S3-06: "Meiji milk 2L" was priced with a smaller pack, "Nescafe Gold 200g" at
   S$2.49, and qty 2 became 1): the product you pick must have the listed size / weight / pack count; note its title,
   size and price as shown on the page; multiply by the qty column; totals with calculate. No matching size → say so.
   **A shop's own search finds nothing, or the shop blocks you** (Cloudflare "You have been blocked", robot check): do not
   conclude the product is not sold there. Search the web once — web_search("<product> site:<shop domain>", e.g.
   "Dyson V15 Detect site:courts.com.sg") — and browser_read the product page it returns. Still nothing → say "not
   verified at <shop>" (not "not sold"). Only use shop domains you have seen in search results; never guess one.
   **"The cheapest X"**: sorting by lowest price (Amazon `&s=price-asc-rank`) lists accessories first (cases, covers,
   cables "for Anker …"). Keep only results whose title is the product itself (brand + product type + spec, e.g. "Anker
   Power Bank 20000mAh"), using browser_find("<brand> <type> <spec>") to get those links with prices; if a sorted page
   shows only accessories, drop the sort and pick the cheapest matching item from the normal results instead of
   re-filtering the same URL.
2. **See the results.** Shop pages are long; the text snapshot is cut off. Do NOT re-open the same URL.
   **Ranking or filtering the results** ("the best-rated under S$25", "the cheapest", "the top 3 with 200+ reviews") needs
   EVERY result on the page, not the first screen (2026-10-30: three cases were ranked "best" from the first screen while
   better-rated ones sat further down). browser_read(url) gives the whole page's text — every product with its price,
   rating and number of ratings; or browser_scroll("down") + browser_look until the footer. Write the candidates down,
   apply every condition (exact phone model, a case and not an accessory, price, number of ratings; skip "Sponsored"
   only if it fails one), then rank — and say how many results you compared.
   - browser_look("List the products on screen with title, price, rating and the label of each product link") — the vision model
     sees the real page, including images.
   - browser_find("<product words>") finds product links anywhere on the page with their price in `context`.
   - browser_scroll("down") then browser_look again for more results.
3. **Choose.** Match the user's wishes (e.g. "looks nice" → ask browser_look which ones look most attractive and why; check it fits
   the exact model, e.g. iPhone 17 Pro Max, not 17 Pro). Prefer good ratings with many reviews; skip "Sponsored" unless it fits best.
   The user wants to choose themselves → show the candidates as cards with present_choices (see "Comparing and watching").
4. **Open the product page** (click its title link), then find the button: browser_find("Add to Cart") (Amazon also uses
   "Add to Basket"); an option the request names (colour/size) is selected by code on a known shop (its form function);
   on another shop the product page goes to the browser-use skill.
5. **Add to cart** by clicking the button's ref. Adding to the cart is not a purchase and doesn't need approval.
   Checkout only when the user explicitly asked to buy; the click that pays or places the order (Buy Now, Place order,
   Pay) is always the user's — hand the filled page over (below).
6. **Verify**: browser_look("Was the item added to the cart? What does the confirmation say and how many items are in the cart?")
   or check the cart count in the snapshot.
7. **Login / CAPTCHA / robot check** → first finish everything that needs no login (look up every item on the list,
   prices, totals, budget checks — product pages are public), then browser_request_takeover for the cart / checkout step
   and say what is already done. Do not stop the whole task at the first "Log in" button.
   Never type passwords. Shipping and card details at checkout are filled by code (step 8), otherwise the user enters them in the hand-over; the final Pay / Place order click is approved separately.
8. **Buying (the user asked you to actually order)**: delivery to the user's home unless they say otherwise.
   - The checkout form is filled by code on known sites — shipping name / phone / address from the profile, the card from
     the Vault after purchase_confirm (form_fill is called by code, not by you). A site with no form function goes to the
     `browser-use` skill.
   - Go through checkout up to the page that shows the final total (items + shipping + tax) and the delivery address.
     "Proceed to checkout", "Next step" and "Continue to payment" only move between pages and need no approval; do not
     stop to ask the user about them.
   - Then call **purchase_confirm ONCE**: site, items (name, qty, price as shown), shipping, total, currency, delivery
     ("Home delivery to <address>, <ETA>"), card_item_id (vault_list → the card the user keeps for shopping, when it is
     complete). After it code fills the card. **The Place order / Pay click is the user's** (Sentinel refuses it): with
     everything filled — a card saved by name only, or no card, leaves the card boxes empty — call browser_request_takeover ("Check the order, type the card if it is empty and press
     Place order yourself; hand the browser back when it is done").
   - If the total, the item, the card or the site changes, call purchase_confirm again.
   - After the hand-back: read the confirmation page and report the order number, items, total and delivery date; no
     confirmation → the order was not placed, say so.
9. **Final answer**: product title, price, seller/rating, link, and "added to cart ✓ (not checked out)"; mention anything uncertain.

## Comparing and watching
- Showing several candidates — and always when the user wants to pick ("show me options", "let me choose", "I'll pick
  one"): call present_choices(kind="comparison") with labels/prices copied exactly from the page (e.g. from
  browser_find context or browser_read). The user picks by pressing a card, so a plain list in the answer is not enough.
  OMuse checks them against what you read; put your opinion in note. Listings from a results page with criteria
  ("best-rated under S$25 with 200+ ratings"): pass ranking with those criteria and every listing of the page as an option
  with its price, stars and ratings as numbers (price_text as the page writes it) — OMuse picks and orders them. The pick arrives as "I choose: <label>": carry on
  with what the user asked for it (e.g. add it to the cart).
- "Tell me when it's cheaper / back in stock": watch_create(url=product page, mode="price_below", threshold=…, keyword=
  a word from the product's own title, current_price=the price you saw) or mode="text", text="In stock". It notifies once
  per new drop; no need for a schedule. The watch reads the page itself and says what it read — report THAT value to the
  user; if it refuses (it read a different price), fix the keyword or say the page can't be watched reliably.

## After the order / cancelling or returning
- The order lands in the ledger (Trust page → Ledger) with the order number from the confirmation page — always report
  that number. To cancel or return later, orders_list first (an empty query lists every order; the ledger names the
  shop by its domain) and the shop's confirmation email (gmail_search the order number or the shop) for the order page's
  link; act on that exact order.
- **The final button is the user's** (Place order, Pay, Request cancellation, Return, Refund): Sentinel refuses that
  click. Open the order's cancel / return page; its form (reason, items, refund method as the user said) is filled in
  the `browser-use` skill; then call browser_request_takeover("press <the button's label> to cancel order <number>") — do not stop with "press
  it yourself" in the answer without the takeover. After the hand-back, browser_snapshot the page and report what it
  says: the order number, cancellation / return requested or not, the refund amount and when it arrives, any reference.
