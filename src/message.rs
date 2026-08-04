/*
 * Copyright (c) Radical HQ Limited
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use crate::{
    error::{Error, Result},
    output::output,
};

pub type MessageSectionsMap =
    std::collections::BTreeMap<MessageSection, String>;

const PULL_REQUEST_TRAILER: &str = "SPR-Pull-Request";

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
pub enum MessageSection {
    Title,
    Summary,
    TestPlan,
    Reviewers,
    ReviewedBy,
    PullRequest,
    Trailers,
}

pub fn message_section_label(section: &MessageSection) -> &'static str {
    use MessageSection::*;

    match section {
        Title => "Title",
        Summary => "Summary",
        TestPlan => "Test Plan",
        Reviewers => "Reviewers",
        ReviewedBy => "Reviewed By",
        PullRequest => PULL_REQUEST_TRAILER,
        Trailers => "Trailers",
    }
}

pub fn message_section_by_label(label: &str) -> Option<MessageSection> {
    use MessageSection::*;

    match &label.to_ascii_lowercase()[..] {
        "title" => Some(Title),
        "summary" => Some(Summary),
        "test plan" => Some(TestPlan),
        "reviewer" => Some(Reviewers),
        "reviewers" => Some(Reviewers),
        "reviewed by" => Some(ReviewedBy),
        _ => None,
    }
}

pub fn parse_message(
    msg: &str,
    top_section: MessageSection,
) -> MessageSectionsMap {
    let (msg, trailer_block) = if top_section == MessageSection::Title {
        split_trailer_block(msg)
    } else {
        (msg, None)
    };
    let regex = lazy_regex::regex!(r#"^\s*([\w\s]+?)\s*:\s*(.*)$"#);

    let mut section = top_section;
    let mut lines_in_section = Vec::<&str>::new();
    let mut sections =
        std::collections::BTreeMap::<MessageSection, String>::new();

    for (lineno, line) in msg
        .trim()
        .split('\n')
        .map(|line| line.trim_end())
        .enumerate()
    {
        if let Some(caps) = regex.captures(line) {
            let label = caps.get(1).unwrap().as_str();
            let payload = caps.get(2).unwrap().as_str();

            if let Some(new_section) = message_section_by_label(label) {
                append_to_message_section(
                    sections.entry(section),
                    lines_in_section.join("\n").trim(),
                );
                section = new_section;
                lines_in_section = vec![payload];
                continue;
            }
        }

        if lineno == 0 && top_section == MessageSection::Title {
            sections.insert(top_section, line.to_string());
            section = MessageSection::Summary;
        } else {
            lines_in_section.push(line);
        }
    }

    if !lines_in_section.is_empty() {
        append_to_message_section(
            sections.entry(section),
            lines_in_section.join("\n").trim(),
        );
    }

    if let Some(trailer_block) = trailer_block {
        parse_trailer_block(trailer_block, &mut sections);
    }

    sections
}

fn split_trailer_block(msg: &str) -> (&str, Option<&str>) {
    let msg = msg.trim_end();
    let Some((body, candidate)) = msg.rsplit_once("\n\n") else {
        return (msg, None);
    };
    let trailer =
        lazy_regex::regex!(r#"^([A-Za-z0-9][A-Za-z0-9_-]*)\s*[:=].*$"#);
    let mut saw_trailer = false;

    for line in candidate.lines() {
        if line.starts_with([' ', '\t']) && saw_trailer {
            continue;
        }
        let Some(captures) = trailer.captures(line) else {
            return (msg, None);
        };
        let token = captures.get(1).unwrap().as_str();
        if message_section_by_label(token).is_some() {
            return (msg, None);
        }
        saw_trailer = true;
    }

    if saw_trailer {
        (body, Some(candidate))
    } else {
        (msg, None)
    }
}

fn parse_trailer_block(trailer_block: &str, sections: &mut MessageSectionsMap) {
    let trailer =
        lazy_regex::regex!(r#"^([A-Za-z0-9][A-Za-z0-9_-]*)\s*[:=]\s*(.*)$"#);
    let mut pull_request_values = Vec::<String>::new();
    let mut other_lines = Vec::<&str>::new();
    let mut parsing_pull_request = false;

    for line in trailer_block.lines() {
        if let Some(captures) = trailer.captures(line) {
            let token = captures.get(1).unwrap().as_str();
            parsing_pull_request =
                token.eq_ignore_ascii_case(PULL_REQUEST_TRAILER);
            if parsing_pull_request {
                pull_request_values
                    .push(captures.get(2).unwrap().as_str().to_string());
            } else {
                other_lines.push(line);
            }
        } else if parsing_pull_request {
            let value = pull_request_values.last_mut().unwrap();
            value.push('\n');
            value.push_str(line);
        } else {
            other_lines.push(line);
        }
    }

    for value in pull_request_values {
        append_to_message_section(
            sections.entry(MessageSection::PullRequest),
            &value,
        );
    }
    if !other_lines.is_empty() {
        sections.insert(MessageSection::Trailers, other_lines.join("\n"));
    }
}

fn append_to_message_section(
    entry: std::collections::btree_map::Entry<MessageSection, String>,
    text: &str,
) {
    if !text.is_empty() {
        entry
            .and_modify(|value| {
                if value.is_empty() {
                    *value = text.to_string();
                } else {
                    *value = format!("{}\n\n{}", value, text);
                }
            })
            .or_insert_with(|| text.to_string());
    } else {
        entry.or_default();
    }
}

pub fn build_message(
    section_texts: &MessageSectionsMap,
    sections: &[MessageSection],
) -> String {
    let mut result = String::new();
    let mut display_label = false;

    for section in sections {
        let value = section_texts.get(section);
        if let Some(text) = value {
            if !result.is_empty() {
                result.push('\n');
            }

            if section != &MessageSection::Title
                && section != &MessageSection::Summary
            {
                // Once we encounter a section that's neither Title nor Summary,
                // we start displaying the labels.
                display_label = true;
            }

            if display_label {
                let label = message_section_label(section);
                result.push_str(label);
                result.push_str(
                    if label.len() + text.len() > 76 || text.contains('\n') {
                        ":\n"
                    } else {
                        ": "
                    },
                );
            }

            result.push_str(text);
            result.push('\n');
        }
    }

    result
}

pub fn build_commit_message(section_texts: &MessageSectionsMap) -> String {
    let mut message = build_message(
        section_texts,
        &[
            MessageSection::Title,
            MessageSection::Summary,
            MessageSection::TestPlan,
            MessageSection::Reviewers,
            MessageSection::ReviewedBy,
        ],
    );
    append_trailer_block(&mut message, section_texts);
    message
}

pub fn build_github_body(section_texts: &MessageSectionsMap) -> String {
    build_message(
        section_texts,
        &[MessageSection::Summary, MessageSection::TestPlan],
    )
}

pub fn build_github_body_for_merging(
    section_texts: &MessageSectionsMap,
) -> String {
    let mut message = build_message(
        section_texts,
        &[
            MessageSection::Summary,
            MessageSection::TestPlan,
            MessageSection::Reviewers,
            MessageSection::ReviewedBy,
        ],
    );
    append_trailer_block(&mut message, section_texts);
    message
}

fn append_trailer_block(
    message: &mut String,
    section_texts: &MessageSectionsMap,
) {
    let pull_request = section_texts.get(&MessageSection::PullRequest);
    let trailers = section_texts.get(&MessageSection::Trailers);
    if pull_request.is_none() && trailers.is_none() {
        return;
    }

    if !message.is_empty() {
        message.push('\n');
    }
    if let Some(pull_request) = pull_request {
        message.push_str(PULL_REQUEST_TRAILER);
        message.push_str(": ");
        message.push_str(pull_request);
        message.push('\n');
    }
    if let Some(trailers) = trailers {
        message.push_str(trailers.trim_end());
        message.push('\n');
    }
}

pub fn validate_commit_message(
    message: &MessageSectionsMap,
    config: &crate::config::Config,
) -> Result<()> {
    if config.require_test_plan
        && !message.contains_key(&MessageSection::TestPlan)
    {
        output("💔", "Commit message does not have a Test Plan!")?;
        return Err(Error::empty());
    }

    let title_missing_or_empty = match message.get(&MessageSection::Title) {
        None => true,
        Some(title) => title.is_empty(),
    };
    if title_missing_or_empty {
        output("💔", "Commit message does not have a title!")?;
        return Err(Error::empty());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    #[test]
    fn test_parse_empty() {
        assert_eq!(
            parse_message("", MessageSection::Title),
            [(MessageSection::Title, "".to_string())].into()
        );
    }

    #[test]
    fn test_parse_title() {
        assert_eq!(
            parse_message("Hello", MessageSection::Title),
            [(MessageSection::Title, "Hello".to_string())].into()
        );
        assert_eq!(
            parse_message("Hello\n", MessageSection::Title),
            [(MessageSection::Title, "Hello".to_string())].into()
        );
        assert_eq!(
            parse_message("\n\nHello\n\n", MessageSection::Title),
            [(MessageSection::Title, "Hello".to_string())].into()
        );
    }

    #[test]
    fn test_parse_title_and_summary() {
        assert_eq!(
            parse_message("Hello\nFoo Bar", MessageSection::Title),
            [
                (MessageSection::Title, "Hello".to_string()),
                (MessageSection::Summary, "Foo Bar".to_string())
            ]
            .into()
        );
        assert_eq!(
            parse_message("Hello\n\nFoo Bar", MessageSection::Title),
            [
                (MessageSection::Title, "Hello".to_string()),
                (MessageSection::Summary, "Foo Bar".to_string())
            ]
            .into()
        );
        assert_eq!(
            parse_message("Hello\n\n\nFoo Bar", MessageSection::Title),
            [
                (MessageSection::Title, "Hello".to_string()),
                (MessageSection::Summary, "Foo Bar".to_string())
            ]
            .into()
        );
        assert_eq!(
            parse_message("Hello\n\nSummary:\nFoo Bar", MessageSection::Title),
            [
                (MessageSection::Title, "Hello".to_string()),
                (MessageSection::Summary, "Foo Bar".to_string())
            ]
            .into()
        );
    }

    #[test]
    fn test_parse_sections() {
        assert_eq!(
            parse_message(
                r#"Hello

Test plan: testzzz

Summary:
here is
the
summary (it's not a "Test plan:"!)

Reviewer:    a, b, c"#,
                MessageSection::Title
            ),
            [
                (MessageSection::Title, "Hello".to_string()),
                (
                    MessageSection::Summary,
                    "here is\nthe\nsummary (it's not a \"Test plan:\"!)"
                        .to_string()
                ),
                (MessageSection::TestPlan, "testzzz".to_string()),
                (MessageSection::Reviewers, "a, b, c".to_string()),
            ]
            .into()
        );
    }

    #[test]
    fn test_parse_and_build_trailers() {
        let message = r#"Fix parser

Keep PR metadata.

Test Plan: unit

SPR-Pull-Request: https://github.com/acme/codez/pull/123
Co-authored-by: Helper <helper@example.com>
Signed-off-by: Author <author@example.com>
 continuation"#;
        let sections = parse_message(message, MessageSection::Title);

        assert_eq!(
            sections
                .get(&MessageSection::PullRequest)
                .map(String::as_str),
            Some("https://github.com/acme/codez/pull/123")
        );
        assert_eq!(
            sections.get(&MessageSection::Trailers).map(String::as_str),
            Some(
                "Co-authored-by: Helper <helper@example.com>\n\
                 Signed-off-by: Author <author@example.com>\n continuation"
            )
        );
        assert_eq!(build_commit_message(&sections), format!("{message}\n"));

        let github_body = build_github_body(&sections);
        assert!(!github_body.contains("SPR-Pull-Request"));
        assert!(!github_body.contains("Co-authored-by"));

        let merge_body = build_github_body_for_merging(&sections);
        assert!(!merge_body.contains("Fix parser"));
        assert!(merge_body.contains("SPR-Pull-Request"));
        assert!(merge_body.contains("Co-authored-by"));
    }

    #[test]
    fn test_does_not_parse_legacy_pull_request_section() {
        let sections = parse_message(
            r#"Fix parser

Test Plan: unit

Pull Request: https://github.com/acme/codez/pull/123"#,
            MessageSection::Title,
        );

        assert!(!sections.contains_key(&MessageSection::PullRequest));
    }

    #[test]
    fn test_does_not_parse_trailers_from_github_body() {
        let sections = parse_message(
            "Description\n\nFixes: #123",
            MessageSection::Summary,
        );

        assert!(!sections.contains_key(&MessageSection::Trailers));
        assert_eq!(
            sections.get(&MessageSection::Summary).map(String::as_str),
            Some("Description\n\nFixes: #123")
        );
    }
}
