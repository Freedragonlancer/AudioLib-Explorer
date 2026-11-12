// crates/audiolib-ai/src/provider/jev_http_tests.rs
//! Exercise Jev's actual HTTP route with local, request-derived typed responses.
use super::*;
use std::{io::Write, net::TcpListener, thread};

fn read_request(stream: &mut std::net::TcpStream) -> (String, Value) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut head = Vec::new();
    let mut byte = [0];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
        assert!(head.len() < 8192);
    }
    let header = String::from_utf8(head).unwrap();
    let length: usize = header
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|value| value.trim().parse().unwrap())
        })
        .unwrap_or(0);
    assert!(length < 128 * 1024);
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (header, body)
}

fn reply(stream: &mut std::net::TcpStream, status: &str, body: &Value) {
    let body = body.to_string();
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
}

fn decision(body: &Value, selected: &str) -> Value {
    let mut answers = serde_json::Map::new();
    for (key, question) in body["questions"].as_object().unwrap() {
        let answer = match question["type"].as_str().unwrap() {
            "choice" => {
                let probabilities: serde_json::Map<String, Value> = question["criteria"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(|id| (id.clone(), json!(if id == selected { 1.0 } else { 0.0 })))
                    .collect();
                assert!(probabilities.contains_key(selected));
                json!({"type":"choice","choice":selected,"confidence":1.0,"probabilities":probabilities})
            }
            "score" => {
                if key == &format!("fit_{selected}") {
                    json!({"type":"score","score":1.8,"confidence":0.8,"probabilities":{"0":0.0,"1":0.2,"2":0.8}})
                } else {
                    json!({"type":"score","score":0.0,"confidence":1.0,"probabilities":{"0":1.0,"1":0.0,"2":0.0}})
                }
            }
            "noul" => json!({"type":"noul","noul":0.1}),
            other => panic!("Unexpected Jev question type: {other}"),
        };
        answers.insert(key.clone(), answer);
    }
    json!({"model":"jev-1.14.0","answers":answers,"usage":{"input_tokens":80,"output_tokens":40}})
}

#[test]
fn jev_sends_wind_keywords_ucs_and_recipes_through_the_typed_endpoint() {
    for (intent, kind, expected_text, layers) in [
        ("wind", "keyword", "wind", false),
        ("我要找远处的风声", "keyword", "wind", false),
        ("EXPLDsgn", "ucs", "EXPLDsgn", false),
        ("科幻爆炸", "recipe", "scifi explosion", true),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let profile = Profile {
            protocol: Protocol::Jev,
            base_url: format!("http://{}/v1", listener.local_addr().unwrap()),
            model: "jev-latest".into(),
            ..Default::default()
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (header, body) = read_request(&mut stream);
            assert!(header.starts_with("POST /v1/systemone HTTP/1.1"));
            assert!(
                header
                    .to_ascii_lowercase()
                    .contains("authorization: bearer fixture-key")
            );
            assert_eq!(body["model"], "jev-latest");
            assert_eq!(body["state"]["intent"], intent);
            let state = body["state"].as_object().unwrap();
            assert_eq!(state.len(), 2);
            let criteria = body["questions"]["recipe"]["criteria"].as_object().unwrap();
            assert!(criteria.len() <= 255);
            assert!(criteria.contains_key("other_or_unclear"));
            let selected = body["state"]["candidates"]
                .as_object()
                .unwrap()
                .iter()
                .find(|(_, candidate)| {
                    candidate["kind"] == kind
                        && candidate["queries"]
                            .as_array()
                            .is_some_and(|queries| queries.iter().any(|q| q == expected_text))
                })
                .map(|(id, _)| id.clone())
                .expect("Expected candidate must reach the API");
            reply(&mut stream, "200 OK", &decision(&body, &selected));
            selected
        });
        let output = generate(
            &profile,
            &ApiKey::new("fixture-key".into()).unwrap(),
            intent,
            layers,
            crate::config::DEFAULT_PROMPT,
            "fixture",
            &AtomicBool::new(false),
        )
        .unwrap();
        let selected = server.join().unwrap();
        assert_eq!(output.plan.intent, intent);
        assert!(
            output
                .plan
                .queries
                .iter()
                .any(|query| query.text == expected_text)
        );
        assert!(
            output
                .plan
                .queries
                .iter()
                .all(|query| query.source_ids == [selected.clone()])
        );
        assert_eq!((output.input_tokens, output.output_tokens), (80, 40));
        let evidence = output.decision.unwrap();
        assert_eq!(evidence.recipe, selected);
        assert_eq!(evidence.probability, 1.0);
    }
}

#[test]
fn jev_other_is_uncertain_and_http_failure_is_not_a_successful_probe() {
    for status in ["200 OK", "401 Unauthorized", "429 Too Many Requests"] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let profile = Profile {
            protocol: Protocol::Jev,
            base_url: format!("http://{}/v1", listener.local_addr().unwrap()),
            model: "jev-latest".into(),
            ..Default::default()
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (_, body) = read_request(&mut stream);
            reply(&mut stream, status, &decision(&body, "other_or_unclear"));
        });
        let result = generate(
            &profile,
            &ApiKey::default(),
            "wind",
            false,
            crate::config::DEFAULT_PROMPT,
            "fixture",
            &AtomicBool::new(false),
        );
        server.join().unwrap();
        assert!(match status {
            "200 OK" => matches!(result, Err(AiError::Uncertain)),
            "401 Unauthorized" => matches!(result, Err(AiError::Http(401))),
            _ => matches!(result, Err(AiError::Http(429))),
        });
    }
}

#[test]
fn jev_model_discovery_accepts_the_typesafe_named_model_catalogue() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let profile = Profile {
        protocol: Protocol::Jev,
        base_url: format!("http://{}/v1", listener.local_addr().unwrap()),
        ..Default::default()
    };
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let (header, body) = read_request(&mut stream);
        assert!(header.starts_with("GET /v1/models HTTP/1.1"));
        assert!(body.is_null());
        reply(
            &mut stream,
            "200 OK",
            &json!({"models":[{"name":"jev-latest"},{"name":"jev-preview"},{"name":"jev-latest"}]}),
        );
    });
    let names = models(&profile, &ApiKey::default(), &AtomicBool::new(false)).unwrap();
    assert_eq!(names, ["jev-latest", "jev-preview"]);
    server.join().unwrap();
}
