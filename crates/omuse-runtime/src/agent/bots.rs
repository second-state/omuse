//! Bots in the executor (`crate::bots`): the skills and prompt of the bot a task's conversation belongs to, and the
//! hand-off from the main bot to a specialised one.
//!
//! `ask_bot(bot, question, conversation_id?)` opens a conversation in that bot (or continues the one given), posts the
//! question there and suspends the main task in `WAITING_BOT`. The bot's task starts once the main task is suspended
//! ([`Runtime::suspend`]), so it can never finish before anyone waits for it. When the bot's task ends
//! ([`Runtime::handoff_done`], from `set_status`), the main task resumes and ends at once with the bot's answer, kept
//! word for word in the main conversation with a link (`meta.handoff`) to the bot's conversation. An approval or a
//! takeover the bot asks for is shown in the main conversation too ([`Runtime::mirror_to_parent`]).
//!
//! Which bot answers is decided before the executor's first turn, by the planner model ([`Runtime::route`]): it reads
//! the request, the recent conversation and the bots, and names the bot, a self-contained question for it (the
//! request plus what the bot needs from this conversation) and, for a follow-up, the bot conversation to continue. The
//! code checks the decision (a known bot; a conversation of that bot started from this one) and hands over at once. When
//! the planner keeps the request in Main, the executor answers without `ask_bot`; only when the planner fails does the
//! executor get `ask_bot` and decide itself.

use futures::future::BoxFuture;
use omuse_common::pyjson;
use omuse_common::util::{get_str, now_ts, py_str, take_chars, truncate, truthy};
use serde_json::{json, Value};

use super::lang::agent_lang;
use super::runtime::{opts, Runtime, Suspend};
use super::util::sget;
use crate::bots::{self, Bot};
use crate::llm::{extract_json, ChatOpts, ToolCall};

/// The status of a main-bot task that waits for a specialised bot's answer.
pub const WAITING_BOT: &str = "WAITING_BOT";

/// The prompt section of a bot: its own instructions; for the main bot also the list of the other bots and the rules
/// for handing over.
pub fn bot_prompt(bot: &Bot, en: bool) -> String {
    let mut out = if en {
        format!("\n## You are the {} bot\n{}", bot.name, bot.prompt)
    } else {
        format!("\n## 你是「{}」助手\n{}", bot.label(false), bot.prompt)
    };
    if bot.is_main() {
        let lines: Vec<String> = bots::all()
            .iter()
            .filter(|b| !b.is_main())
            .map(|b| {
                if en {
                    format!("- {} — {}: {}", b.id, b.name, b.description)
                } else {
                    format!(
                        "- {} — {}（{}）：{}",
                        b.id,
                        b.label(false),
                        b.name,
                        if b.description_zh.is_empty() {
                            &b.description
                        } else {
                            &b.description_zh
                        }
                    )
                }
            })
            .collect();
        out += &if en {
            format!("\n\n## Bots\nSpecialised bots — each has its own skills; all share the user's memory, profile, Vault and connectors:\n{}\n\
A request that belongs to one of them goes to it with ask_bot(bot, question): ONE call, alone in its turn, and nothing \
else — do not answer it yourself and do not look for its skills (you don't have them). question = the user's request \
in full plus what the bot needs from this conversation (dates, names, places, what was already decided): the bot does \
not see this chat. The bot's answer is shown to the user word for word with a link to the bot's conversation, and your \
task ends there. A follow-up on something a bot already answered here (the history marks it \"[answered by the … bot \
in conversation conv_…]\") goes back to that bot with conversation_id = that conversation, so it keeps its context. A \
request for two bots: hand it to the bot for the main part and put the rest in question as context. Everything else — \
general questions, email, news, research, shopping, documents, calls — you answer yourself.",
                lines.join("\n"))
        } else {
            format!("\n\n## 专门助手\n下面这些专门助手各有自己的技能；记忆、档案、保险箱和连接器都是共用的：\n{}\n\
属于其中某个助手的请求，用 ask_bot(bot, question) 交给它：只调用一次，这一轮只放这一个调用，别的什么都不做——不要自己回答，\
也不要去找它的技能（你没有）。question = 用户的完整请求，加上这个助手需要的本对话里的信息（日期、姓名、地点、已经定下的事）：\
它看不到这段对话。助手的回答会原样显示给用户，并附上它那段对话的链接，你的任务到此结束。用户追问某个助手已经在这里回答过的事\
（历史里标着「[answered by the … bot in conversation conv_…]」）时，用 conversation_id = 那段对话交回同一个助手，它会记得上下文。\
一个请求涉及两个助手时，交给负责主要部分的那个，其余写进 question 作为背景。其他请求（一般问题、邮件、新闻、调研、购物、文件、\
电话）你自己回答。",
                lines.join("\n"))
        };
    } else {
        out += if en {
            "\nYour skills are the ones listed above. A request outside them (another bot's work): say in one line which \
bot handles it — the user can ask in Main, or open that bot from the menu."
        } else {
            "\n你的技能就是上面列出的那些。超出这些范围的请求（属于别的助手）：用一句话说明该找哪个助手——用户可以在主助手里问，\
或从菜单打开那个助手。"
        };
    }
    out
}

/// The planner's routing decision for a main-bot request.
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// Hand over: the `ask_bot` arguments (`bot`, `question`, `conversation_id` when continuing).
    Bot(Value),
    /// Main answers itself.
    Main,
    /// The planner gave no usable answer: the executor decides (with `ask_bot`).
    Unknown,
}

/// The router's system prompt: the bots and the rules for the JSON answer.
pub fn route_prompt() -> String {
    let lines: Vec<String> = bots::all()
        .iter()
        .filter(|b| !b.is_main())
        .map(|b| format!("- {} — {}: {}", b.id, b.name, b.description))
        .collect();
    format!("## Route\nYou route a request the user sent to OMuse's main bot. Specialised bots (each has its own skills; \
all share the user's memory, profile, Vault and connectors):\n{}\n\
- main — everything else: general questions, writing, email, news, research, shopping, documents, calls, schedules.\n\n\
Answer with ONE JSON object and nothing else: {{\"bot\": \"<id>\", \"question\": \"…\", \"conversation_id\": \"…\"}}.\n\
- bot: the bot whose work the request is; for a request that needs two bots, the one for the main part. \"main\" when \
no specialised bot fits.\n\
- question (not for main): the request for that bot, standing on its own, because the bot does not see this \
conversation: the user's words, plus what the bot needs from the conversation (dates, places, names, what was \
already decided or answered). Keep every condition the user gave exactly; add none they did not give (no budget or \
price limit, no number of travellers, no airline, no dates) — the bot has the profile and asks if it must. Write it \
in the language the user wrote in.\n\
- conversation_id: only for a follow-up on something a bot already answered in this conversation (the history marks \
it \"[answered by the <bot> bot in conversation conv_…]\"): that conversation, so the bot keeps its context. Leave it \
out otherwise.", lines.join("\n"))
}

/// The routing decision as the code accepts it: a specialised bot that exists, a non-empty question (the request
/// itself when the planner left it out), and a conversation to continue only when it is that bot's and was started from
/// `parent_conv`; anything else is [`Route::Unknown`] (or [`Route::Main`] for `main`).
pub fn checked_route(raw: &Value, goal: &str, conv_ok: impl Fn(&str, &str) -> bool) -> Route {
    if !raw.is_object() {
        return Route::Unknown;
    }
    let want = sget(raw, "bot").trim().to_lowercase();
    if want == bots::MAIN {
        return Route::Main;
    }
    let Some(bot) = bots::get(&want).filter(|b| !b.is_main()) else {
        return Route::Unknown;
    };
    let mut question = sget(raw, "question").trim().to_string();
    if question.is_empty() {
        question = goal.trim().to_string();
    }
    let mut args = json!({"bot": bot.id, "question": question});
    let cid = sget(raw, "conversation_id").trim().to_string();
    if !cid.is_empty() && conv_ok(&cid, &bot.id) {
        args["conversation_id"] = json!(cid);
    }
    Route::Bot(args)
}

impl Runtime {
    /// The main bot's first step: the planner model picks the bot for this request (see the module docs).
    pub(crate) async fn route(&self, t: &Value, history: &[Value]) -> Route {
        let tid = get_str(t, "id");
        let parent = get_str(t, "conv_id");
        let goal = get_str(t, "goal");
        let recent: Vec<String> = history[history.len().saturating_sub(12)..]
            .iter()
            .map(|m| {
                format!(
                    "{}: {}",
                    get_str(m, "role"),
                    take_chars(&m.get("content").map(py_str).unwrap_or_default(), 800)
                )
            })
            .collect();
        let user = format!(
            "Today: {}\n\nConversation so far:\n{}\n\nThe user's new request:\n{}",
            chrono::Utc::now().format("%Y-%m-%d"),
            if recent.is_empty() {
                "(none)".to_string()
            } else {
                recent.join("\n")
            },
            goal
        );
        let msgs = [
            json!({"role": "system", "content": route_prompt()}),
            json!({"role": "user", "content": user}),
        ];
        let pm = sget(&self.settings(), "planner_model");
        let o = ChatOpts {
            max_tokens: Some(1200),
            temperature: Some(0.0),
            model: if pm.is_empty() { None } else { Some(pm) },
            no_think: true,
            ..opts("router", &tid)
        };
        let raw = match self.chat_or_cancel(&tid, &msgs, None, &o).await {
            Some(Ok(r)) => extract_json(&r.content).unwrap_or(Value::Null),
            Some(Err(e)) => {
                self.event(
                    &tid,
                    "route_error",
                    json!({"message": take_chars(&e.to_string(), 300)}),
                )
                .await;
                Value::Null
            }
            None => Value::Null,
        };
        let route = checked_route(&raw, &goal, |cid, bot| {
            self.store.conv(cid).ok().flatten().is_some_and(|c| {
                get_str(&c, "bot_id") == bot && get_str(&c, "parent_conv") == parent
            })
        });
        let ev = match &route {
            Route::Bot(a) => {
                json!({"bot": a.get("bot"), "conversation_id": a.get("conversation_id"),
                                    "question": take_chars(&sget(a, "question"), 500)})
            }
            Route::Main => json!({"bot": bots::MAIN}),
            Route::Unknown => json!({"bot": null, "raw": take_chars(&pyjson::dumps(&raw), 300)}),
        };
        self.event(&tid, "route", ev).await;
        route
    }

    /// The bot of the conversation a task belongs to.
    pub fn bot_of(&self, t: &Value) -> &'static Bot {
        bots::get_or_main(&self.store.conv_bot(&get_str(t, "conv_id")))
    }

    /// The enabled skills this bot may use.
    pub fn bot_skills(&self, bot: &Bot) -> Vec<Value> {
        bots::filter_skills(bot, self.skills())
    }

    /// `ask_bot`: post the question in the bot's conversation and suspend the main task until the bot answers.
    /// `Err` is the tool error for the model.
    pub(crate) async fn ask_bot(
        &self,
        t: &Value,
        args: &Value,
        call: &ToolCall,
        remaining: &[ToolCall],
    ) -> Result<Suspend, String> {
        let me = self.bot_of(t);
        if !me.is_main() {
            return Err(format!(
                "ERROR: only the main bot hands requests to other bots; you are the {} bot. Answer with your own \
skills, or say which bot the user should ask.",
                me.name
            ));
        }
        let ids: Vec<&str> = bots::all()
            .iter()
            .filter(|b| !b.is_main())
            .map(|b| b.id.as_str())
            .collect();
        let want = sget(args, "bot").trim().to_lowercase();
        let Some(bot) = bots::get(&want).filter(|b| !b.is_main()) else {
            return Err(format!(
                "ERROR: unknown bot {want:?}. Use one of: {}.",
                ids.join(", ")
            ));
        };
        let question = sget(args, "question").trim().to_string();
        if question.is_empty() {
            return Err("ERROR: question is empty: give the user's request in full, with what the bot needs from this conversation.".into());
        }
        let parent_conv = get_str(t, "conv_id");
        let parent_tid = get_str(t, "id");
        let mut cid = sget(args, "conversation_id").trim().to_string();
        if !cid.is_empty() {
            let ok = self
                .store
                .conv(&cid)
                .map_err(|e| format!("ERROR: {e}"))?
                .is_some_and(|c| get_str(&c, "bot_id") == bot.id);
            if !ok {
                return Err(format!(
                    "ERROR: {cid} is not a conversation of the {} bot. Leave conversation_id out to start a new one.",
                    bot.name
                ));
            }
        } else {
            cid = self
                .store
                .create_conv(take_chars(&question, 40), "chat", None)
                .map_err(|e| format!("ERROR: {e}"))?;
            self.store
                .set_conv_bot(&cid, &bot.id, &parent_conv)
                .map_err(|e| format!("ERROR: {e}"))?;
        }
        let meta = json!({"from": {"conv_id": parent_conv, "task_id": parent_tid}});
        let child = (|| -> anyhow::Result<Value> {
            let mid = self
                .store
                .add_msg(&cid, "user", &question, "", Some(&meta))?;
            let child = self.store.create_task(
                &question,
                &cid,
                "chat",
                "",
                "",
                t.get("attachments").filter(|a| truthy(a)),
            )?;
            self.store.db.execute(
                "UPDATE messages SET task_id=? WHERE id=?",
                omuse_common::params![get_str(&child, "id").as_str(), mid],
            )?;
            Ok(child)
        })()
        .map_err(|e| format!("ERROR: {e}"))?;
        let child_tid = get_str(&child, "id");
        self.publish(json!({"kind": "task_update", "task": Self::task_brief(&child)}))
            .await;
        self.publish(json!({"kind": "conv_update", "conv_id": cid}))
            .await;
        self.audit(
            "runtime",
            "task.create",
            &child_tid,
            json!({"detail": {"goal": truncate(&question, 500), "source": "chat", "bot": bot.id, "from_task": parent_tid}}),
        )
        .await;
        let info = json!({"bot": bot.id, "bot_name": bot.name, "bot_name_zh": bot.name_zh, "icon": bot.icon,
                          "conv_id": cid, "task_id": child_tid});
        self.event(&parent_tid, "handoff", info.clone()).await;
        let mut calls = vec![call.clone()];
        calls.extend(remaining.iter().cloned());
        let mut waiting = info;
        waiting["type"] = json!("bot");
        waiting["_pending"] = json!({"call_id": call.id, "calls": calls});
        Ok(Suspend {
            status: WAITING_BOT.into(),
            waiting,
        })
    }

    /// A task ended: if a main-bot task waits for it, resume that task with the answer.
    /// (Boxed: `set_status` spawns it and it calls `set_status`.)
    pub fn handoff_done<'a>(&'a self, child_tid: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let Some(child) = self.store.task(child_tid).ok().flatten() else {
                return;
            };
            for p in self
                .store
                .tasks(Some(WAITING_BOT), 100, None)
                .unwrap_or_default()
            {
                let w = p.get("waiting").cloned().unwrap_or(Value::Null);
                if sget(&w, "task_id") != child_tid {
                    continue;
                }
                let ptid = get_str(&p, "id");
                let Some(full) = self.store.task(&ptid).ok().flatten() else {
                    continue;
                };
                let mut pend = full
                    .get("pending")
                    .filter(|x| x.is_object())
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let status = get_str(&child, "status");
                let result = json!({"status": "ok", "child_status": status, "answer": get_str(&child, "result"),
                                "error": get_str(&child, "error")});
                pend["resolved"] = json!({"call_id": pend.get("call_id"), "decision": "bot_done", "result": result});
                let _ = self.store.update_task(&ptid, &json!({"pending": pend}));
                self.event(
                &ptid,
                "handoff_done",
                json!({"bot": w.get("bot"), "conv_id": w.get("conv_id"), "task_id": child_tid, "status": status}),
            )
            .await;
                self.set_status(&ptid, "RUNNING", json!({"waiting": null}))
                    .await;
                self.start(&ptid);
            }
        })
    }

    /// The main task resumes after `ask_bot`: it ends at once with the bot's answer, word for word, and a link to the
    /// bot's conversation (`meta.handoff`).
    pub(crate) async fn finish_handoff(
        &self,
        t: &Value,
        waiting: &Value,
        result: &Value,
    ) -> anyhow::Result<()> {
        let tid = get_str(t, "id");
        let conv_id = get_str(t, "conv_id");
        let en = agent_lang(&self.settings()) == "en";
        let bot = bots::get_or_main(&sget(waiting, "bot"));
        let name = bot.label(en);
        let status = sget(result, "child_status");
        let answer = sget(result, "answer").trim().to_string();
        let (text, final_status) = match status.as_str() {
            "COMPLETED" => (answer, "COMPLETED"),
            "CANCELLED" => (
                if en {
                    format!("The {name} bot's task was cancelled, so there is no answer.")
                } else {
                    format!("「{name}」助手的任务已取消，没有回答。")
                },
                "CANCELLED",
            ),
            _ => {
                let why = sget(result, "error");
                let head = if en {
                    format!(
                        "The {name} bot did not finish{}.",
                        if why.is_empty() {
                            String::new()
                        } else {
                            format!(" ({why})")
                        }
                    )
                } else {
                    format!(
                        "「{name}」助手没有完成{}。",
                        if why.is_empty() {
                            String::new()
                        } else {
                            format!("（{why}）")
                        }
                    )
                };
                (
                    if answer.is_empty() {
                        head
                    } else {
                        format!("{head}\n\n{answer}")
                    },
                    "FAILED",
                )
            }
        };
        let meta = json!({"handoff": {"bot": bot.id, "bot_name": bot.name, "bot_name_zh": bot.name_zh, "icon": bot.icon,
                                      "conv_id": waiting.get("conv_id"), "task_id": waiting.get("task_id")}});
        self.store.update_task(
            &tid,
            &json!({"pending": null, "result": text, "finished_at": now_ts()}),
        )?;
        let kw = if final_status == "FAILED" {
            json!({"error": sget(result, "error")})
        } else {
            json!({})
        };
        self.set_status(&tid, final_status, kw).await;
        self.event(&tid, "final", json!({"text": truncate(&text, 4000)}))
            .await;
        self.store
            .add_msg(&conv_id, "assistant", &text, &tid, Some(&meta))?;
        self.publish(json!({"kind": "conv_update", "conv_id": conv_id}))
            .await;
        let me = self.arc();
        let (cid, t2) = (conv_id.clone(), tid.clone());
        tokio::spawn(async move { me.retitle(&cid, &t2).await });
        Ok(())
    }

    /// A bot's task stopped for an approval or a takeover: show the same card in the main conversation that handed it
    /// over, so the user can act without leaving Main.
    pub(crate) async fn mirror_to_parent(&self, cid: &str, card: &Value, task_id: &str) {
        let parent = self
            .store
            .conv(cid)
            .ok()
            .flatten()
            .map(|c| get_str(&c, "parent_conv"))
            .unwrap_or_default();
        if parent.is_empty() {
            return;
        }
        let mut card = card.clone();
        card["from_bot"] = json!(self.store.conv_bot(cid));
        card["bot_conv"] = json!(cid);
        if self
            .store
            .add_msg(&parent, "system", &pyjson::dumps(&card), task_id, None)
            .is_ok()
        {
            self.publish(json!({"kind": "conv_update", "conv_id": parent}))
                .await;
        }
    }

    /// The main task was cancelled while it waited for a bot: cancel the bot's task too.
    pub(crate) async fn cancel_handoff(&self, waiting: &Value, reason: &str) {
        let child = sget(waiting, "task_id");
        if sget(waiting, "type") == "bot" && !child.is_empty() {
            Box::pin(self.cancel(&child, reason)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_prompt_lists_every_specialised_bot_and_main() {
        let p = route_prompt();
        for b in bots::all().iter().filter(|b| !b.is_main()) {
            assert!(p.contains(&format!("- {} — {}", b.id, b.name)), "{}", b.id);
        }
        assert!(p.contains("- main — everything else") && p.contains("add none they did not give"));
    }

    #[test]
    fn the_routing_decision_is_checked_in_code() {
        let ok = |cid: &str, bot: &str| cid == "conv_kid" && bot == "travel";
        let r = checked_route(
            &json!({"bot": " Travel ", "question": "SIN to TYO on 2026-12-01", "conversation_id": "conv_kid"}),
            "g",
            ok,
        );
        assert_eq!(
            r,
            Route::Bot(
                json!({"bot": "travel", "question": "SIN to TYO on 2026-12-01", "conversation_id": "conv_kid"})
            )
        );
        // a conversation that is not that bot's (or not from this one) is dropped: a new one
        let r = checked_route(
            &json!({"bot": "cards", "question": "q", "conversation_id": "conv_kid"}),
            "g",
            ok,
        );
        assert_eq!(r, Route::Bot(json!({"bot": "cards", "question": "q"})));
        // no question: the request itself; a merged bot's old id is the bot it was merged into
        let r = checked_route(&json!({"bot": "cities"}), " what to see in Osaka ", ok);
        assert_eq!(
            r,
            Route::Bot(json!({"bot": "travel", "question": "what to see in Osaka"}))
        );
        assert_eq!(
            checked_route(&json!({"bot": "main", "question": "x"}), "g", ok),
            Route::Main
        );
        for raw in [
            json!({"bot": "nope"}),
            json!({}),
            Value::Null,
            json!("travel"),
            json!({"bot": ""}),
        ] {
            assert_eq!(checked_route(&raw, "g", ok), Route::Unknown, "{raw}");
        }
    }

    #[test]
    fn main_prompt_lists_the_other_bots_and_the_hand_over_rules() {
        let main = bots::get_or_main(bots::MAIN);
        let en = bot_prompt(main, true);
        assert!(en.contains("## You are the Main bot") && en.contains("## Bots"));
        assert!(en.contains("- travel — Travel:") && en.contains("- photos — Photos & videos:"));
        assert!(!en.contains("- main —"));
        assert!(
            en.contains("ask_bot(bot, question): ONE call")
                && en.contains("conversation_id = that conversation")
        );
        let zh = bot_prompt(main, false);
        assert!(zh.contains("## 专门助手") && zh.contains("- travel — 旅行（Travel）"));
        let travel = bot_prompt(bots::get("travel").unwrap(), true);
        assert!(travel.contains("## You are the Travel bot") && !travel.contains("## Bots"));
        assert!(travel.contains("say in one line which bot handles it"));
    }
}
