"""City guides with a real model, from the user's point of view (tests/user_common.py, tests/travel2_common.py): the twelve
installed guides city_guides_real.py does not ask — Singapore, Bangkok, Seoul, Kuala Lumpur, Taipei, Hong Kong, Ho Chi
Minh City, Beijing, Shenzhen, Guangzhou, Chengdu and Osaka — each answer a visitor's question about their own city from
their own content. The user opens the Travel bot from the menu and asks; the answer carries facts the guide holds (often
recent ones a model's memory may not have: the Circle Line loop of July 2026, Bangkok's common ticket for 2027, Seoul's
₩1,550 fare of June 2025, KL's LRT3 of June 2026, the guide's stations, lines and cards) and comes without the user being
asked to approve anything or to take over a browser. Nothing inside the system is looked at. With city_guides_real.py
(Shanghai, Tokyo, Hanoi) this covers every installed city guide; it replaces the scripted per-city answers of
city_guides_e2e.py. One TIMING line per question.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh city_guides2_real"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import travel2_common as T2  # noqa: E402
from tests import user_common as U  # noqa: E402

CASES = [   # (city, question, facts — one must be there, more facts — one must be there)
    ("singapore", "I'm visiting Singapore from the US next month. Can I just tap my Visa card on the MRT and buses, and is the Circle "
                  "Line a full loop yet?",
     [r"12 July|July 12|July 2026"], [r"contactless|tap", r"SimplyGo"]),
    ("bangkok", "In Bangkok, can I use one ticket across the BTS Skytrain and the MRT, and can I tap my credit card at the gates?",
     [r"2027", r"common ticket"], [r"Rabbit"]),
    ("seoul", "How much does a subway ride cost in Seoul, and what's the easiest way to pay as a visitor?",
     [r"1,?550"], [r"T-?money", r"WOWPASS"]),
    ("kuala-lumpur", "Can I tap my bank card at the train gates in Kuala Lumpur, or do I need a card? And has the new LRT line to Shah "
                     "Alam opened?",
     [r"LRT ?3|Shah Alam line|Johan Setia"], [r"Touch ?'?n ?Go"]),
    ("taipei", "Can I drink water or chew gum on the Taipei MRT, and how should I pay for rides and the public bikes?",
     [r"no eating|not even water|no (food|drink)|eat(ing)? (or|and) drink(ing)? (is|are) (banned|prohibited|not allowed)|fine"],
     [r"EasyCard"]),
    ("hong-kong", "First trip to Hong Kong: how do I get around, and can I tap my Visa card on the MTR?",
     [r"Octopus"], [r"Star Ferry", r"minibus", r"ding ding|tram"]),
    ("ho-chi-minh-city", "Is there a metro in Ho Chi Minh City now, and can I rent a motorbike there with my US driver's licence?",
     [r"Line 1", r"Bến Thành|Ben Thanh|Suối Tiên|Suoi Tien"], [r"1968"]),
    ("beijing", "I'm taking the fast train from Beijing to Shanghai. Which station does it leave from, how long does it take, and how do I "
                "book as a foreigner?",
     [r"Beijing South|北京南"], [r"passport"]),
    ("shenzhen", "I'm staying in Shenzhen for business. How long is the fast train to Hong Kong and where do I catch it, and how do I pay "
                 "for the metro?",
     [r"Futian|福田|Shenzhen North|深圳北"], [r"Alipay|WeChat|Shenzhen Tong|深圳通"]),
    ("guangzhou", "I'm going to the Canton Fair. How do I get from Baiyun airport to the fair by metro?",
     [r"Line 3"], [r"Line 8", r"Pazhou|琶洲"]),
    ("chengdu", "In Chengdu I want to see the pandas and take a day trip to Chongqing by train. How do I get around?",
     [r"Chengdu East|成都东"], [r"Line 3", r"panda base|Giant Panda Breeding|Research Base"]),
    ("osaka", "Staying in Osaka for a week. What's the best way to do day trips to Kyoto and Nara by train, and which transit card do I get?",
     [r"Hankyu|Keihan|Kintetsu"], [r"ICOCA"]),
]

U.setup()
U.run(lambda: T2.guides(CASES))
