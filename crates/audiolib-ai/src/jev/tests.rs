// crates/audiolib-ai/src/jev/tests.rs
//! Jev wire-contract and deterministic candidate-selection regressions.
use super::*;

fn score(value: f64) -> Value {
    let upper = value.min(1.0);
    let probabilities = if value <= 1.0 {
        json!({"0":1.0-upper,"1":upper,"2":0.0})
    } else {
        json!({"0":0.0,"1":2.0-value,"2":value-1.0})
    };
    json!({"type":"score","score":value,"confidence":0.8,"probabilities":probabilities})
}

fn response(ids: &[String], choice: &str) -> Value {
    let probability = 0.92;
    let probabilities: BTreeMap<_, _> = ids
        .iter()
        .map(|id| {
            (
                id.clone(),
                if id == choice {
                    probability
                } else {
                    (1.0 - probability) / (ids.len() - 1) as f64
                },
            )
        })
        .collect();
    let confidence = (probability - 1.0 / ids.len() as f64) / (1.0 - 1.0 / ids.len() as f64);
    let mut answers = serde_json::Map::new();
    answers.insert(
        "recipe".into(),
        json!({"type":"choice","choice":choice,"confidence":confidence,"probabilities":probabilities}),
    );
    answers.insert(
        "needs_clarification".into(),
        json!({"type":"noul","noul":0.1}),
    );
    for id in ids.iter().filter(|id| id.as_str() != OTHER) {
        answers.insert(fit_key(id), score(1.8));
    }
    json!({"answers":answers})
}

fn request_for(intent: &str, layers: bool) -> (Value, Vec<String>) {
    request("jev-latest", intent, layers, crate::config::DEFAULT_PROMPT).unwrap()
}

#[test]
fn finite_selection_supports_keywords_ucs_and_recipes() {
    for (intent, id, expected_query) in [
        ("wind", "P0300", "wind"),
        ("WINDGust", "WINDGust", "WINDGust"),
        ("科幻爆炸", "scifi-explosion", "scifi explosion"),
    ] {
        let (body, ids) = request_for(intent, true);
        assert!(ids.iter().any(|candidate| candidate == id), "{intent}");
        let (plan, decision) = parse(&response(&ids, id), &ids, intent, true).unwrap();
        assert!(plan.queries.iter().any(|q| q.text == expected_query));
        assert_eq!(decision.recipe, id);
        assert_eq!(decision.probability, 0.92);
        assert_ne!(decision.confidence, decision.probability);
        let candidate = &body["state"]["candidates"][id];
        assert!(candidate["kind"].is_string());
        assert!(candidate["title"].is_string());
        assert!(candidate["queries"].is_array());
        assert!(candidate["note"].is_string());
        assert!(
            body["questions"]["recipe"]["instructions"]
                .as_str()
                .unwrap()
                .contains(crate::config::DEFAULT_PROMPT)
        );
    }
}

#[test]
fn selected_candidate_uses_its_own_independent_fit() {
    let (_, ids) = request_for("科幻爆炸", false);
    let choice = "scifi-explosion";
    let mut value = response(&ids, choice);
    value["answers"][fit_key(choice)] = score(0.4);
    assert!(matches!(
        parse(&value, &ids, "科幻爆炸", false),
        Err(AiError::Uncertain)
    ));
    for id in ids.iter().filter(|id| id.as_str() != OTHER) {
        value["answers"][fit_key(id)] = score(if id == choice { 1.8 } else { 0.0 });
    }
    let (_, decision) = parse(&value, &ids, "科幻爆炸", false).unwrap();
    assert_eq!(decision.fit, 1.8);
}

#[test]
fn material_conflict_does_not_override_the_selected_candidate_fit() {
    let intent = "木地板上的脚步";
    let (_, ids) = request_for(intent, false);
    let mut value = response(&ids, "P0120");
    value["answers"][fit_key("footsteps")] = score(0.0);
    let (plan, _) = parse(&value, &ids, intent, false).unwrap();
    assert!(plan.queries.iter().any(|query| query.text == "footsteps"));
    assert!(
        plan.queries
            .iter()
            .all(|query| !query.text.contains("gravel"))
    );

    let mut value = response(&ids, "footsteps");
    value["answers"][fit_key("footsteps")] = score(0.0);
    assert!(matches!(
        parse(&value, &ids, intent, false),
        Err(AiError::Uncertain)
    ));
}

#[test]
fn unknown_intent_still_has_bounded_candidates_and_an_other_outcome() {
    let (body, ids) = request_for("qzxqzxqzxqzxq", false);
    assert!(ids.len() >= 20);
    assert!(ids.len() <= MAX_CHOICE_OPTIONS);
    assert_eq!(body["questions"].as_object().unwrap().len(), ids.len() + 1);
    assert!(matches!(
        parse(&response(&ids, OTHER), &ids, "qzxqzxqzxqzxq", false),
        Err(AiError::Uncertain)
    ));
}

#[test]
fn response_rejects_wrong_types_missing_extra_and_invalid_answers() {
    let (_, ids) = request_for("wind", false);
    let choice = "P0300";
    let baseline = response(&ids, choice);
    let mut cases = Vec::new();
    let mut value = baseline.clone();
    value["answers"]
        .as_object_mut()
        .unwrap()
        .remove(&fit_key(choice));
    cases.push(value);
    let mut value = baseline.clone();
    value["answers"]["unrequested"] = score(2.0);
    cases.push(value);
    let mut value = baseline.clone();
    value["answers"]["needs_clarification"] = json!({"type":"noul"});
    cases.push(value);
    let mut value = baseline.clone();
    value["answers"]["recipe"]["choice"] = json!("invented-id");
    cases.push(value);
    let mut value = baseline.clone();
    value["answers"]["recipe"]["choice"] = json!(OTHER);
    cases.push(value);
    let mut value = baseline.clone();
    value["answers"]["recipe"]["probabilities"][choice] = json!(0.2);
    cases.push(value);
    let mut value = baseline.clone();
    value["answers"]["recipe"]["confidence"] = json!(1.1);
    cases.push(value);
    let mut value = baseline.clone();
    value["answers"]["needs_clarification"]["noul"] = json!(-0.1);
    cases.push(value);
    let mut value = baseline.clone();
    value["answers"][fit_key(choice)] = json!({"type":"noul","noul":0.9});
    cases.push(value);
    let mut value = baseline.clone();
    value["answers"][fit_key(choice)]["score"] = json!(3.0);
    cases.push(value);
    let mut value = baseline.clone();
    value["answers"][fit_key(choice)]["score"] = json!(1.2);
    cases.push(value);
    let mut value = baseline.clone();
    value["answers"][fit_key(choice)]["probabilities"] = json!({"0":0.0,"1":0.2});
    cases.push(value);
    let mut value = baseline.clone();
    value["answers"][fit_key(choice)]["probabilities"] = json!({"0":-0.1,"1":0.3,"2":0.8});
    cases.push(value);
    let mut value = baseline.clone();
    value["answers"]["recipe"]["probabilities"]
        .as_object_mut()
        .unwrap()
        .remove(OTHER);
    cases.push(value);
    let mut value = baseline.clone();
    let selected_score = value["answers"]
        .as_object_mut()
        .unwrap()
        .remove(&fit_key(choice))
        .unwrap();
    value["answers"]["fit_invented-id"] = selected_score;
    cases.push(value);
    let mut value = baseline.clone();
    let unselected = ids
        .iter()
        .find(|id| id.as_str() != choice && id.as_str() != OTHER)
        .unwrap();
    value["answers"][fit_key(unselected)]["confidence"] = json!(null);
    cases.push(value);
    for value in cases {
        assert!(matches!(
            parse(&value, &ids, "wind", false),
            Err(AiError::Response)
        ));
    }
    let mut value = baseline;
    value["answers"]["needs_clarification"]["noul"] = json!(0.8);
    assert!(matches!(
        parse(&value, &ids, "wind", false),
        Err(AiError::Uncertain)
    ));
}

#[test]
fn option_bounds_include_other_and_reject_duplicate_ids() {
    let mut ids: Vec<String> = (0..254).map(|i| format!("candidate-{i}")).collect();
    ids.push(OTHER.into());
    assert!(option_ids_valid(&ids));
    ids.push("one-too-many".into());
    assert!(!option_ids_valid(&ids));
    let invalid = vec![OTHER.into(), OTHER.into()];
    assert!(!option_ids_valid(&invalid));
    assert!(!option_ids_valid(&["candidate".into()]));
    assert!(!option_ids_valid(&[]));
    assert!(!unit(f64::NAN));
    assert!(!unit(f64::INFINITY));
}

#[test]
fn caller_ids_cannot_authorize_an_unknown_registry_entry() {
    let (_, mut ids) = request_for("wind", false);
    let candidate = ids.iter_mut().find(|id| id.as_str() != OTHER).unwrap();
    *candidate = "invented-id".into();
    assert!(matches!(
        parse(&response(&ids, "invented-id"), &ids, "wind", false),
        Err(AiError::Response)
    ));
    assert!(request("jev-latest", "", false, crate::config::DEFAULT_PROMPT).is_err());
    assert!(
        request(
            "jev-latest",
            &"风".repeat(501),
            false,
            crate::config::DEFAULT_PROMPT
        )
        .is_err()
    );
}

#[test]
fn wide_requests_keep_candidate_context_once_with_utf8_safe_limits() {
    let guidance = "声".repeat(2000);
    let broad = "风雨雷电 火焰 冰 狗猫鸟 水流 脚步 木材 金属 发动机 爆炸";
    let wide_intent = format!("{broad}{}", "声".repeat(500 - broad.chars().count()));
    let full = "chain cloth pat explosion balloon foley scrape scratch scrub squash squeak squeeze stretch stutter squeak squeak sizzle squeeze bomb whistle jingle rustle chatter air blood disembowel gore AIRBlow AIRBrst AIRHiss AIRMisc AIRSuck AERODoor AEROHeli AEROInt AEROJet AEROMech AEROMil AEROMisc AEROProp AERORadio AERORckt ALRMBell ALRMBuzr ALRMClok ALRMElec ALRMMisc ALRMSirn AMBAir AMBAlpn AMBAmus";
    let full_intent = format!("{full}{}", "声".repeat(500 - full.chars().count()));
    for layers in [false, true] {
        for intent in [
            wide_intent.as_str(),
            full_intent.as_str(),
            "wind",
            "EXPLDsgn",
        ] {
            let (body, ids) = request("jev-latest", intent, layers, &guidance).unwrap();
            let body_bytes = serde_json::to_vec(&body).unwrap().len();
            let state_bytes = serde_json::to_vec(&body["state"]).unwrap().len();
            let questions = body["questions"].as_object().unwrap();
            let largest_question_bytes = questions
                .values()
                .map(|question| serde_json::to_vec(question).unwrap().len())
                .max()
                .unwrap();
            eprintln!(
                "Jev request: {} options, {body_bytes} body bytes, {} state + largest-question bytes",
                ids.len(),
                state_bytes + largest_question_bytes
            );
            // Byte budgets catch payload growth; they are not tokenizer or context-length claims.
            assert!(body_bytes <= 96 * 1024, "{intent}: {body_bytes} body bytes");
            assert!(
                state_bytes + largest_question_bytes <= 32 * 1024,
                "{intent}: {} state + largest-question bytes",
                state_bytes + largest_question_bytes
            );
            assert!(ids.len() <= 68);
            if intent == full_intent {
                assert_eq!(ids.len(), 68);
            }
            for (id, candidate) in body["state"]["candidates"].as_object().unwrap() {
                assert!(
                    candidate["title"].as_str().unwrap().chars().count()
                        <= MAX_CANDIDATE_TITLE_CHARS
                );
                assert!(
                    candidate["note"].as_str().unwrap().chars().count() <= MAX_CANDIDATE_NOTE_CHARS
                );
                assert!(body["questions"]["recipe"]["criteria"][id].is_string());
                if id != OTHER {
                    assert_eq!(
                        body["questions"][fit_key(id)]["instructions"]["candidate_id"],
                        *id
                    );
                }
            }
            let decoded: Value =
                serde_json::from_slice(&serde_json::to_vec(&body).unwrap()).unwrap();
            assert_eq!(decoded, body);
        }
    }
}

#[test]
fn candidate_queries_match_selected_plan_in_each_mode() {
    let intent = "科幻爆炸";
    for layers in [false, true] {
        let (body, ids) = request_for(intent, layers);
        assert_eq!(
            body["state"]["mode"],
            if layers { "layers" } else { "direct" }
        );
        let candidates = body["state"]["candidates"].as_object().unwrap();
        let recipes: Vec<_> = candidates
            .iter()
            .filter(|(_, candidate)| candidate["kind"] == "recipe")
            .collect();
        assert_eq!(recipes.len(), 19);
        for (id, candidate) in recipes {
            let (plan, _) = parse(&response(&ids, id), &ids, intent, layers).unwrap();
            let queries: Vec<_> = plan
                .queries
                .iter()
                .map(|query| query.text.as_str())
                .collect();
            assert_eq!(candidate["queries"], json!(queries), "{id} in {layers}");
        }
        let scifi = &candidates["scifi-explosion"];
        let queries = scifi["queries"].as_array().unwrap();
        let note = scifi["note"].as_str().unwrap();
        assert!(!queries.iter().any(|query| query == "reverb tail"));
        assert!(!note.contains("reverb tail"));
        if layers {
            assert_eq!(queries.len(), audiolib_core::ai_search::MAX_AI_QUERIES);
            assert!(queries.iter().any(|query| query == "sub boom"));
        } else {
            assert_eq!(queries.len(), 3);
            assert!(!queries.iter().any(|query| query == "sub boom"));
            assert!(!note.contains("sub boom"));
        }
    }
}
