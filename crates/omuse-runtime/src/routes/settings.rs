//! Settings (with the "use my own model" rules) and the model tests.

use std::sync::{Arc, LazyLock};
use std::time::Instant;

use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::Json;
use omuse_common::api::ApiError;
use omuse_common::util::{get_str, py_str, take_chars, truthy};
use regex::Regex;
use serde_json::{json, Map, Value};

use super::{body_obj, body_opt, round2, St, R};
use crate::llm::{ChatOpts, Llm};
use crate::store::{redacted, system_model, system_options, MODEL_OPTIONS, REASONING_EFFORTS};

/// settings: the values in force (the model key only as model_api_key_set). model_user: the user's own model
/// settings as saved in Settings → Models, whether or not they are switched on.
pub async fn get_settings(st: St) -> R {
    let store = &st.rt.store;
    // names only, no endpoint
    let mut system = super::filter_obj(&system_model(&st.cfg), |k| {
        k.ends_with("_model") || k == "model_name"
    });
    if let (Value::Object(m), Value::Object(opts)) = (&mut system, system_options(&st.cfg)) {
        m.extend(opts); // fast mode and reasoning of the system's models, shown read-only
    }
    let skills: Vec<Value> = st
        .rt
        .skills()
        .iter()
        .map(|s| json!({"name": s["name"], "description": s["description"]}))
        .collect();
    Ok(Json(
        json!({"settings": redacted(&store.settings()?), "model_user": store.user_model()?,
                   "system_model": system, "skills": skills}),
    ))
}

/// `str(v or "")`.
fn s_or_empty(v: Option<&Value>) -> String {
    v.filter(|v| truthy(v)).map(py_str).unwrap_or_default()
}

/// Settings → Models → "use my own model": the endpoint, the key and the LLM are required (a key already saved
/// counts). Normalises the values in place; errors with HTTP 400 and a bilingual message.
pub fn check_model_override(b: &mut Map<String, Value>, saved: &Value) -> Result<(), ApiError> {
    static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^https?://\S+$").unwrap());
    let pick = |k: &str| {
        s_or_empty(if b.contains_key(k) {
            b.get(k)
        } else {
            saved.get(k)
        })
    };
    let url = pick("model_base_url")
        .trim()
        .trim_end_matches('/')
        .to_string();
    let name = pick("model_name").trim().to_string();
    let given_key = s_or_empty(b.get("model_api_key")).trim().to_string();
    let key = if given_key.is_empty() {
        s_or_empty(saved.get("model_api_key"))
    } else {
        given_key.clone()
    };
    if !URL.is_match(&url) {
        return Err(ApiError::new(
            400,
            "接口地址必须是以 http:// 或 https:// 开头的网址 (the endpoint must be an http(s) URL)",
        ));
    }
    if name.is_empty() {
        return Err(ApiError::new(
            400,
            "必须填写执行任务的 LLM 模型名 (the LLM model name is required)",
        ));
    }
    if key.is_empty() {
        return Err(ApiError::new(
            400,
            "必须填写 API key (an API key is required to use your own model)",
        ));
    }
    b.insert("model_base_url".into(), json!(url));
    b.insert("model_name".into(), json!(name));
    if given_key.is_empty() {
        b.shift_remove("model_api_key"); // blank: the saved key stays as it is
    } else {
        b.insert("model_api_key".into(), json!(given_key));
    }
    for k in ["planner_model", "vision_model", "stt_model", "image_model"] {
        if b.contains_key(k) {
            let v = s_or_empty(b.get(k)).trim().to_string();
            b.insert(k.into(), json!(v));
        }
    }
    Ok(())
}

pub async fn put_settings(st: St, body: Bytes) -> R {
    static HM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(?:[01]?\d|2[0-3]):[0-5]\d$").unwrap());
    let b = body_obj(&body)?;
    let mut b = b.as_object().cloned().unwrap_or_default();
    let store = &st.rt.store;
    if let Some(v) = b.get("memory_consolidate_at") {
        if !HM.is_match(py_str(v).trim()) {
            return Err(ApiError::new(400, "整理时间格式应为 HH:MM (time must be HH:MM)").into());
        }
    }
    if b.get("model_override").is_some_and(truthy) {
        b.insert("model_override".into(), json!(true));
        // defaults for fields left out: the user's own values as saved (never the system's)
        let mut saved = store.user_model()?;
        saved["model_api_key"] = json!(s_or_empty(store.saved_settings()?.get("model_api_key")));
        check_model_override(&mut b, &saved)?;
    } else if b.contains_key("model_override") {
        b.insert("model_override".into(), json!(false));
    }
    // fast mode and reasoning belong to the user's own model: the system's come from the config
    let own = match b.get("model_override") {
        Some(v) => truthy(v),
        None => store
            .saved_settings()?
            .get("model_override")
            .is_some_and(truthy),
    };
    if !own {
        for k in MODEL_OPTIONS {
            b.shift_remove(k);
        }
    } else {
        if let Some(v) = b.get("fast_mode") {
            let on = truthy(v);
            b.insert("fast_mode".into(), json!(on));
        }
        if b.contains_key("reasoning_effort") {
            let effort = s_or_empty(b.get("reasoning_effort")).trim().to_lowercase();
            if !REASONING_EFFORTS.contains(&effort.as_str()) {
                return Err(ApiError::new(
                    400,
                    "推理强度只能是 none、minimal、low、medium、high、xhigh 或留空 (reasoning effort must be none, minimal, low, medium, high, xhigh or empty)",
                )
                .into());
            }
            b.insert("reasoning_effort".into(), json!(effort));
        }
    }
    if b.contains_key("model_api_key") && s_or_empty(b.get("model_api_key")).trim().is_empty() {
        b.shift_remove("model_api_key"); // an empty key field means "keep the saved one"
    }
    if b.contains_key("instance_name") {
        // one line, at most 40 characters; blank = the default
        let joined = s_or_empty(b.get("instance_name"))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let name = take_chars(&joined, 40);
        b.insert(
            "instance_name".into(),
            json!(if name.is_empty() { "OMuse" } else { name }),
        );
    }
    let bv = Value::Object(b.clone());
    let s = store.set_settings(&bv)?;
    // the key itself is never logged
    let detail: Map<String, Value> = b
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                if k == "model_api_key" {
                    json!("(set)")
                } else {
                    v.clone()
                },
            )
        })
        .collect();
    st.rt
        .audit("user", "settings.update", "", json!({"detail": detail}))
        .await;
    Ok(Json(json!({"settings": redacted(&s)})))
}

const OK_PROMPT: &str = "只回复 OK 两个字母。Reply with just OK.";

fn test_opts(max_tokens: i64, model: Option<String>) -> ChatOpts {
    ChatOpts {
        purpose: "test".into(),
        max_tokens: Some(max_tokens),
        no_think: true,
        model,
        ..Default::default()
    }
}

pub async fn test_model(st: St) -> R {
    let t0 = Instant::now();
    let msgs = [json!({"role": "user", "content": OK_PROMPT})];
    Ok(Json(
        match st.rt.llm.chat(&msgs, None, &test_opts(400, None)).await {
            Ok(r) => json!({"ok": true, "reply": take_chars(&r.content, 200),
                        "latency_s": round2(t0.elapsed().as_secs_f64())}),
            Err(e) => json!({"ok": false, "error": take_chars(&e.to_string(), 500)}),
        },
    ))
}

/// A 48×48 red PNG as a data URL (the vision test).
fn red_square() -> anyhow::Result<String> {
    use base64::Engine;
    let img = image::RgbImage::from_pixel(48, 48, image::Rgb([220, 30, 30]));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
    ))
}

/// 0.3 s of 16 kHz mono 16-bit silence as a WAV file (the speech-to-text test).
fn silence_wav() -> Vec<u8> {
    let frames: u32 = 4800;
    let data_len = frames * 2;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&16000u32.to_le_bytes());
    w.extend_from_slice(&32000u32.to_le_bytes()); // byte rate
    w.extend_from_slice(&2u16.to_le_bytes()); // block align
    w.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    w.resize(44 + data_len as usize, 0);
    w
}

fn fixed_llm(s: Value) -> Llm {
    Llm::new(Arc::new(move || s.clone()), None, None)
}

/// One result row: `{"kind", "model", "ok", "latency_s", **detail}` or the error.
fn row(kind: &str, model: &str, t0: Instant, res: Result<Value, String>) -> Value {
    let lat = round2(t0.elapsed().as_secs_f64());
    match res {
        Ok(detail) => {
            let mut m = json!({"kind": kind, "model": model, "ok": true, "latency_s": lat});
            if let (Some(o), Some(d)) = (m.as_object_mut(), detail.as_object()) {
                for (k, v) in d {
                    o.insert(k.clone(), v.clone());
                }
            }
            m
        }
        Err(e) => {
            json!({"kind": kind, "model": model, "ok": false, "latency_s": lat, "error": take_chars(&e, 400)})
        }
    }
}

async fn chat_ok(llm: &Llm, model: Option<String>) -> Result<Value, String> {
    let msgs = [json!({"role": "user", "content": OK_PROMPT})];
    let r = llm
        .chat(&msgs, None, &test_opts(400, model))
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({"reply": take_chars(&r.content, 120)}))
}

async fn vision_ok(llm: &Llm, model: Option<String>) -> Result<Value, String> {
    let url = red_square().map_err(|e| e.to_string())?;
    let msgs = [json!({"role": "user", "content": [
        {"type": "text", "text": "What colour is this square? One word."},
        {"type": "image_url", "image_url": {"url": url}}]})];
    let r = llm
        .chat(&msgs, None, &test_opts(200, model))
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({"reply": take_chars(&r.content, 120)}))
}

async fn image_ok(images: &crate::imagegen::ImageClient, s: &Value) -> Result<Value, String> {
    let (base, model) = images.resolve(s).await.map_err(|e| e.to_string())?;
    let listed = match images.list_models(&base, 15.0, Some(s)).await {
        Ok(list) => Some(list.iter().any(|m| get_str(m, "id") == model)),
        Err(_) => None,
    };
    if listed == Some(false) {
        return Err(format!(
            "endpoint reachable, but it does not list {}",
            model
        ));
    }
    Ok(
        json!({"reply": format!("{} @ {}{} — not generated (billed)", model, base,
                               if listed == Some(true) { " (listed)" } else { "" }),
              "resolved_model": model}),
    )
}

/// One standard request per configured model — LLM, planner, vision, speech-to-text — and a reachability check for
/// the image model (a generation would be billed). The body may carry the form's current values, so the user can
/// test before saving.
pub async fn test_models(st: St, headers: HeaderMap, body: Bytes) -> R {
    let b = body_opt(&headers, &body, &["0", ""])?;
    let store = &st.rt.store;
    let mut s = store.saved_settings()?;
    for k in [
        "model_override",
        "model_base_url",
        "model_name",
        "planner_model",
        "vision_model",
        "stt_model",
        "image_model",
        "llm_timeout",
    ] {
        if let Some(v) = b.get(k) {
            s[k] = v.clone();
        }
    }
    let key = s_or_empty(b.get("model_api_key")).trim().to_string();
    if !key.is_empty() {
        s["model_api_key"] = json!(key);
    }
    let s = store.effective_model(&s); // the form's own-model values with the toggle on, else the system's
    let llm = fixed_llm(s.clone());
    let mut results = Vec::new();

    let t0 = Instant::now();
    let r = chat_ok(&llm, None).await;
    results.push(row("llm", &s_or_empty(s.get("model_name")), t0, r));

    let planner = s_or_empty(s.get("planner_model"));
    if !planner.trim().is_empty() {
        let t0 = Instant::now();
        let r = chat_ok(&llm, Some(planner.clone())).await;
        results.push(row("planner", &planner, t0, r));
    } else {
        results.push(json!({"kind": "planner", "model": "", "ok": true, "same_as": "llm"}));
    }

    let vision = s_or_empty(s.get("vision_model"));
    let vmodel = if vision.is_empty() {
        s_or_empty(s.get("model_name"))
    } else {
        vision.clone()
    };
    let t0 = Instant::now();
    let r = vision_ok(
        &llm,
        if vision.is_empty() {
            None
        } else {
            Some(vision)
        },
    )
    .await;
    results.push(row("vision", &vmodel, t0, r));

    let mut stt_model = s_or_empty(s.get("stt_model")).trim().to_string();
    if stt_model.is_empty() {
        stt_model = llm.stt_model().await;
    }
    if !stt_model.is_empty() {
        let mut s_stt = s.clone();
        s_stt["stt_model"] = json!(stt_model);
        let llm_stt = fixed_llm(s_stt);
        let t0 = Instant::now();
        let r = match llm_stt.transcribe(silence_wav(), "silence.wav", "").await {
            Ok(text) => {
                let text = if text.is_empty() {
                    "(no speech)".to_string()
                } else {
                    text
                };
                Ok(json!({"reply": take_chars(&text, 120)}))
            }
            Err(e) => Err(e.to_string()),
        };
        results.push(row("stt", &stt_model, t0, r));
    } else {
        results.push(json!({"kind": "stt", "model": "", "ok": false,
                            "error": "no speech-to-text model: set one, or the endpoint lists none (whisper-like)"}));
    }

    let image = s_or_empty(s.get("image_model"));
    let t0 = Instant::now();
    let r = image_ok(&st.rt.images, &s).await;
    results.push(row(
        "image",
        if image.is_empty() { "(auto)" } else { &image },
        t0,
        r,
    ));
    Ok(Json(json!({"results": results})))
}
