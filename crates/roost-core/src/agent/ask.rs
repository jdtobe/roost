//! The `ask_user` tool: the agent asks the person a question and waits.
//!
//! Three kinds: pick one of a few options, pick several, or type an answer.
//! Pick questions can also take a typed answer of the user's own. This module
//! is the pure part: reading the model's arguments, checking the user's
//! answer, and the text the model gets back. Waiting for the answer happens
//! in the chat manager, the way it waits for an approval.

use serde::Serialize;
use serde_json::{json, Value};

/// The tool's name.
pub const TOOL: &str = "ask_user";

/// Questions one run may ask; after that the model is told to carry on.
pub const MAX_PER_RUN: u32 = 5;

const MAX_QUESTION_CHARS: usize = 500;
pub const MIN_OPTIONS: usize = 2;
pub const MAX_OPTIONS: usize = 8;
const MAX_OPTION_CHARS: usize = 80;
pub const MAX_ANSWER_CHARS: usize = 2000;

/// What the model gets when the user skips.
pub const SKIPPED: &str =
    "The user chose not to answer this question. Carry on without it, or decide for yourself and say what you assumed.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QuestionKind {
    /// Pick one of the options.
    One,
    /// Pick any number of the options.
    Many,
    /// Type an answer.
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub text: String,
    pub kind: QuestionKind,
    pub options: Vec<String>,
    /// Pick questions: the user may type an answer of their own too.
    pub allow_other: bool,
}

/// What the user answered. For a text question the text is `other`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Answer {
    pub picked: Vec<String>,
    pub other: Option<String>,
}

fn clean(s: &str) -> String {
    s.trim().to_string()
}

/// Reads the model's arguments. The error text goes back to the model.
pub fn parse(args: &Value) -> Result<Question, String> {
    let text = clean(
        args.get("question")
            .and_then(Value::as_str)
            .ok_or("question is required")?,
    );
    if text.is_empty() {
        return Err("question must not be empty".into());
    }
    if text.chars().count() > MAX_QUESTION_CHARS {
        return Err(format!(
            "question is too long (at most {MAX_QUESTION_CHARS} characters)"
        ));
    }
    let kind = match args.get("kind").and_then(Value::as_str) {
        Some("one") => QuestionKind::One,
        Some("many") => QuestionKind::Many,
        Some("text") => QuestionKind::Text,
        _ => return Err("kind must be \"one\", \"many\" or \"text\"".into()),
    };
    let options: Vec<String> = match args.get("options") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_str().map(clean).ok_or("options must be strings"))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("options must be a list of strings".into()),
    };
    if kind == QuestionKind::Text {
        if !options.is_empty() {
            return Err("a text question takes no options".into());
        }
    } else {
        if options.len() < MIN_OPTIONS || options.len() > MAX_OPTIONS {
            return Err(format!(
                "give {MIN_OPTIONS} to {MAX_OPTIONS} options for kind \"one\" or \"many\""
            ));
        }
        for (i, o) in options.iter().enumerate() {
            if o.is_empty() || o.chars().count() > MAX_OPTION_CHARS {
                return Err(format!(
                    "each option must be 1 to {MAX_OPTION_CHARS} characters"
                ));
            }
            if options[..i].contains(o) {
                return Err(format!("the option \"{o}\" is listed twice"));
            }
        }
    }
    let allow_other = kind != QuestionKind::Text
        && args
            .get("allow_other")
            .and_then(Value::as_bool)
            .unwrap_or(true);
    Ok(Question {
        text,
        kind,
        options,
        allow_other,
    })
}

/// Whether `answer` is a valid reply to `q`. The window checks this too; the
/// core does not trust it.
pub fn check_answer(q: &Question, a: &Answer) -> Result<(), String> {
    let other = a.other.as_deref().map(str::trim).filter(|o| !o.is_empty());
    if let Some(o) = other {
        if o.chars().count() > MAX_ANSWER_CHARS {
            return Err("the answer is too long".into());
        }
    }
    if a.other.is_some() && other.is_none() {
        return Err("the typed answer is empty".into());
    }
    for (i, p) in a.picked.iter().enumerate() {
        if !q.options.contains(p) {
            return Err(format!("\"{p}\" is not one of the options"));
        }
        if a.picked[..i].contains(p) {
            return Err("an option is picked twice".into());
        }
    }
    match q.kind {
        QuestionKind::Text => {
            if !a.picked.is_empty() || other.is_none() {
                return Err("type an answer".into());
            }
        }
        QuestionKind::One => {
            if a.picked.len() + usize::from(other.is_some()) != 1 {
                return Err("pick one option or type an answer, not both".into());
            }
            if other.is_some() && !q.allow_other {
                return Err("this question takes only the listed options".into());
            }
        }
        QuestionKind::Many => {
            if a.picked.is_empty() && other.is_none() {
                return Err("pick at least one option".into());
            }
            if other.is_some() && !q.allow_other {
                return Err("this question takes only the listed options".into());
            }
        }
    }
    Ok(())
}

/// The result text the model reads. `a` must have passed `check_answer`.
pub fn result_text(q: &Question, a: &Answer) -> String {
    let other = a
        .other
        .as_deref()
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .map(str::to_string);
    let v = match q.kind {
        QuestionKind::Text => json!({"answer": other.unwrap_or_default()}),
        QuestionKind::One => match (a.picked.first(), other) {
            (_, Some(o)) => json!({"answer": o, "other": true}),
            (Some(p), None) => json!({"answer": p}),
            (None, None) => json!({"answer": ""}),
        },
        QuestionKind::Many => match other {
            Some(o) => json!({"answers": a.picked, "other": o}),
            None => json!({"answers": a.picked}),
        },
    };
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(args: Value) -> Result<Question, String> {
        parse(&args)
    }

    fn one() -> Question {
        q(json!({"question": "Which?", "kind": "one", "options": ["A", "B"]})).unwrap()
    }

    fn many() -> Question {
        q(json!({"question": "Which?", "kind": "many", "options": ["A", "B", "C"]})).unwrap()
    }

    fn text() -> Question {
        q(json!({"question": "Name?", "kind": "text"})).unwrap()
    }

    fn picked(p: &[&str]) -> Answer {
        Answer {
            picked: p.iter().map(|s| s.to_string()).collect(),
            other: None,
        }
    }

    fn typed(t: &str) -> Answer {
        Answer {
            picked: Vec::new(),
            other: Some(t.into()),
        }
    }

    #[test]
    fn a_question_is_read_and_trimmed() {
        let x =
            q(json!({"question": "  Which?  ", "kind": "one", "options": [" A ", "B"]})).unwrap();
        assert_eq!(x.text, "Which?");
        assert_eq!(x.options, ["A", "B"]);
        assert!(x.allow_other, "typed answers are allowed unless turned off");
        let x = q(json!({"question": "Which?", "kind": "many", "options": ["A","B"], "allow_other": false})).unwrap();
        assert!(!x.allow_other);
        assert!(!text().allow_other);
    }

    #[test]
    fn bad_questions_say_what_is_wrong() {
        let bad = |v: Value| q(v).unwrap_err();
        assert!(bad(json!({"kind": "text"})).contains("question is required"));
        assert!(bad(json!({"question": " ", "kind": "text"})).contains("empty"));
        assert!(bad(json!({"question": "x".repeat(501), "kind": "text"})).contains("too long"));
        assert!(bad(json!({"question": "q", "kind": "maybe"})).contains("kind must be"));
        assert!(bad(json!({"question": "q", "kind": "one"})).contains("2 to 8"));
        assert!(bad(json!({"question": "q", "kind": "one", "options": ["A"]})).contains("2 to 8"));
        let nine: Vec<String> = (0..9).map(|i| format!("o{i}")).collect();
        assert!(bad(json!({"question": "q", "kind": "many", "options": nine})).contains("2 to 8"));
        assert!(
            bad(json!({"question": "q", "kind": "one", "options": ["A", "A"]}))
                .contains("listed twice")
        );
        assert!(
            bad(json!({"question": "q", "kind": "one", "options": ["A", ""]})).contains("1 to 80")
        );
        assert!(
            bad(json!({"question": "q", "kind": "one", "options": ["A", 3]})).contains("strings")
        );
        assert!(
            bad(json!({"question": "q", "kind": "text", "options": ["A", "B"]}))
                .contains("no options")
        );
    }

    #[test]
    fn answers_are_checked_against_the_question() {
        // one
        assert!(check_answer(&one(), &picked(&["B"])).is_ok());
        assert!(check_answer(&one(), &typed("C, actually")).is_ok());
        assert!(check_answer(&one(), &picked(&[])).is_err());
        assert!(check_answer(&one(), &picked(&["A", "B"])).is_err());
        assert!(check_answer(&one(), &picked(&["Z"])).is_err());
        let both = Answer {
            picked: vec!["A".into()],
            other: Some("x".into()),
        };
        assert!(check_answer(&one(), &both).is_err());
        // many
        assert!(check_answer(&many(), &picked(&["A", "C"])).is_ok());
        assert!(check_answer(&many(), &picked(&[])).is_err());
        assert!(check_answer(&many(), &picked(&["A", "A"])).is_err());
        let mix = Answer {
            picked: vec!["A".into()],
            other: Some("and more".into()),
        };
        assert!(check_answer(&many(), &mix).is_ok());
        assert!(check_answer(&many(), &typed("only mine")).is_ok());
        // text
        assert!(check_answer(&text(), &typed("Ada")).is_ok());
        assert!(check_answer(&text(), &typed("  ")).is_err());
        assert!(check_answer(&text(), &picked(&["A"])).is_err());
        assert!(check_answer(&text(), &Answer::default()).is_err());
        // limits
        assert!(check_answer(&text(), &typed(&"x".repeat(2001))).is_err());
        // a typed answer where none is allowed
        let strict =
            q(json!({"question": "q", "kind": "one", "options": ["A","B"], "allow_other": false}))
                .unwrap();
        assert!(check_answer(&strict, &typed("C")).is_err());
        assert!(check_answer(&strict, &picked(&["A"])).is_ok());
    }

    #[test]
    fn the_model_reads_the_answer_as_json() {
        assert_eq!(result_text(&one(), &picked(&["B"])), r#"{"answer":"B"}"#);
        assert_eq!(
            result_text(&one(), &typed("  C  ")),
            r#"{"answer":"C","other":true}"#
        );
        assert_eq!(
            result_text(&many(), &picked(&["A", "C"])),
            r#"{"answers":["A","C"]}"#
        );
        let mix = Answer {
            picked: vec!["A".into()],
            other: Some("also D".into()),
        };
        assert_eq!(
            result_text(&many(), &mix),
            r#"{"answers":["A"],"other":"also D"}"#
        );
        assert_eq!(result_text(&text(), &typed("Ada")), r#"{"answer":"Ada"}"#);
        // Quotes and newlines are escaped, not spliced in.
        let r = result_text(&text(), &typed("a \"b\"\nc"));
        assert_eq!(
            serde_json::from_str::<Value>(&r).unwrap()["answer"],
            "a \"b\"\nc"
        );
    }

    #[test]
    fn the_schema_requires_a_question_and_a_kind() {
        let spec = crate::agent::tool::find(TOOL).unwrap().spec();
        let p = spec.parameters;
        assert_eq!(p["required"], json!(["question", "kind"]));
        assert_eq!(
            p["properties"]["kind"]["enum"],
            json!(["one", "many", "text"])
        );
        assert!(spec.description.contains("secrets"));
    }
}
