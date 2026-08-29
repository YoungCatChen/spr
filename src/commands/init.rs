/*
 * Copyright (c) Radical HQ Limited
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use indoc::formatdoc;
use lazy_regex::regex;
use octocrab::FromResponse;
use secrecy::ExposeSecret as _;

use crate::{
    error::{Error, Result, ResultExt},
    output::output,
};

pub async fn init() -> Result<()> {
    output("👋", "Welcome to spr!")?;

    let path = std::env::current_dir()?;
    let repo = git2::Repository::discover(path.clone()).reword(formatdoc!(
        "Could not open a Git repository in {:?}. Please run 'spr' from within \
         a Git repository.",
        path
    ))?;
    let mut config = repo.config()?;

    // Name of remote

    console::Term::stdout().write_line("")?;

    output(
        "❓",
        &formatdoc!(
            "What's the name of the Git remote pointing to GitHub? Usually it's
             'origin'."
        ),
    )?;

    let remote = dialoguer::Input::<String>::new()
        .with_prompt("Name of remote for GitHub")
        .with_initial_text(
            config
                .get_string("spr.githubRemoteName")
                .ok()
                .unwrap_or_else(|| "origin".to_string()),
        )
        .interact_text()?;
    config.set_str("spr.githubRemoteName", &remote)?;

    let remote_url = repo.find_remote(&remote)?.url().map(String::from);
    let inferred_host = remote_url
        .as_deref()
        .and_then(github_remote_details)
        .map(|(host, _)| host);
    let github_host = config
        .get_string("spr.githubHost")
        .ok()
        .or(inferred_host)
        .unwrap_or_else(|| "github.com".to_string());
    let github_host = dialoguer::Input::<String>::new()
        .with_prompt("GitHub host")
        .with_initial_text(github_host)
        .validate_with(|input: &String| validate_github_host(input))
        .interact_text()?;
    let github_host = normalize_github_host(&github_host);
    config.set_str("spr.githubHost", &github_host)?;

    // GitHub Personal Access Token

    let github_auth_token = config
        .get_string("spr.githubAuthToken")
        .ok()
        .and_then(|value| if value.is_empty() { None } else { Some(value) });
    let is_enterprise = github_host != "github.com";
    let valid_auth = if let Some(token) = github_auth_token.as_deref() {
        let client = rest_client_for_host(&github_host, token)?;
        if is_enterprise {
            client.current().user().await.is_ok()
        } else {
            let scopes = client
                .get::<AuthScopes, _, _>("/", Some(&()))
                .await
                .map(|response| response.scopes)
                .unwrap_or_default();
            scopes.iter().any(|s| s == "repo")
                && scopes.iter().any(|s| s == "user")
                && scopes.iter().any(|s| s == "org" || s == "read:org")
        }
    } else {
        false
    };

    let github_auth_token = if valid_auth {
        github_auth_token.unwrap()
    } else if is_enterprise {
        prompt_for_personal_access_token(
            &github_host,
            github_auth_token.as_deref(),
        )?
    } else {
        console::Term::stdout().write_line("")?;

        let client_id = "Ov23liD6WOMYlLy12wkg";
        let client = crate::http_client::new_octocrab(
            Some("https://github.com"),
            None,
            &[(
                http::HeaderName::from_static("accept"),
                "application/json".into(),
            )],
        )?;
        let device_codes = client
            .authenticate_as_device(&client_id.into(), ["repo user read:org"])
            .await?;

        open::that_detached(&device_codes.verification_uri)?;
        output(
        "🔑",
        &formatdoc!("
            Okay, let's get started.

            To authenticate spr with GitHub, please go to

            -----> {} <-----

            and enter code

            > > > > > {} < < < < <

            For your convenience, the link should open in your web browser now.",
            &device_codes.verification_uri,
            &device_codes.user_code,
            )
        )?;

        let auth = device_codes
            .poll_until_available(&client, &client_id.into())
            .await?;
        let token: String = auth.access_token.expose_secret().into();
        token
    };
    config.set_str("spr.githubAuthToken", &github_auth_token)?;

    let octocrab = rest_client_for_host(&github_host, &github_auth_token)?;
    let github_user = octocrab.current().user().await?;

    output("👋", &formatdoc!("Hello {}!", github_user.login))?;

    // Name of the GitHub repo

    console::Term::stdout().write_line("")?;

    output(
        "❓",
        &formatdoc!(
            "What's the name of the GitHub repository. Please enter \
             'OWNER/REPOSITORY' (basically the bit that follow \
             '{github_host}/' in the address.)"
        ),
    )?;

    let github_repo = config
        .get_string("spr.githubRepository")
        .ok()
        .and_then(|value| if value.is_empty() { None } else { Some(value) })
        .or_else(|| {
            remote_url
                .as_deref()
                .and_then(github_remote_details)
                .filter(|(host, _)| host.eq_ignore_ascii_case(&github_host))
                .map(|(_, repository)| repository)
        })
        .unwrap_or_default();

    let github_repo = dialoguer::Input::<String>::new()
        .with_prompt("GitHub repository")
        .with_initial_text(github_repo)
        .interact_text()?;
    config.set_str("spr.githubRepository", &github_repo)?;

    // Master branch name (just query GitHub)

    let github_repo_info = octocrab
        .get::<octocrab::models::Repository, _, _>(
            format!("/repos/{}", &github_repo),
            None::<&()>,
        )
        .await
        .context("Getting github repo info".to_string())?;

    config.set_str(
        "spr.githubMasterBranch",
        github_repo_info
            .default_branch
            .as_ref()
            .map(|s| &s[..])
            .unwrap_or("master"),
    )?;

    // Pull Request branch prefix

    console::Term::stdout().write_line("")?;

    let branch_prefix = config
        .get_string("spr.branchPrefix")
        .ok()
        .and_then(|value| if value.is_empty() { None } else { Some(value) })
        .unwrap_or_else(|| format!("spr/{}/", &github_user.login));

    output(
        "❓",
        &formatdoc!(
            "What prefix should be used when naming Pull Request branches?
             Good practice is to begin with 'spr/' as a general namespace \
             for spr-managed Pull Request branches. Continuing with the \
             GitHub user name is a good idea, so there is no danger of names \
             clashing with those of other users.
             The prefix should end with a good separator character (like '/' \
             or '-'), since commit titles will be appended to this prefix."
        ),
    )?;

    let branch_prefix = dialoguer::Input::<String>::new()
        .with_prompt("Branch prefix")
        .with_initial_text(branch_prefix)
        .validate_with(|input: &String| -> Result<()> {
            validate_branch_prefix(input)
        })
        .interact_text()?;

    config.set_str("spr.branchPrefix", &branch_prefix)?;

    Ok(())
}

fn normalize_github_host(host: &str) -> String {
    host.trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

fn validate_github_host(host: &str) -> Result<()> {
    let host = normalize_github_host(host);
    if host.is_empty()
        || !regex!(r"^[A-Za-z0-9.-]+(?::[0-9]+)?$").is_match(&host)
    {
        return Err(Error::new(
            "GitHub host must be a hostname with an optional port",
        ));
    }
    Ok(())
}

fn github_remote_details(url: &str) -> Option<(String, String)> {
    let captures = regex!(
        r"^(?:https://|ssh://)?(?:git@)?([\w.-]+(?::\d+)?)[/:]([\w.-]+/[\w.-]+?)(?:\.git)?/?$"
    )
    .captures(url)?;
    Some((
        captures.get(1)?.as_str().to_string(),
        captures.get(2)?.as_str().to_string(),
    ))
}

fn rest_client_for_host(
    github_host: &str,
    auth_token: &str,
) -> Result<octocrab::Octocrab> {
    let base_uri = (github_host != "github.com")
        .then(|| format!("https://{github_host}/api/v3"));
    crate::http_client::new_octocrab(base_uri.as_deref(), Some(auth_token), &[])
}

fn prompt_for_personal_access_token(
    github_host: &str,
    existing_token: Option<&str>,
) -> Result<String> {
    console::Term::stdout().write_line("")?;
    output(
        "🔑",
        &formatdoc!(
            "GitHub Enterprise requires a personal access token. Create one at
             https://{github_host}/settings/tokens with repo, user, and
             read:org access, then paste it below."
        ),
    )?;
    let token = dialoguer::Password::new()
        .with_prompt(if existing_token.is_some() {
            "GitHub PAT (leave empty to keep the existing token)"
        } else {
            "GitHub Personal Access Token"
        })
        .allow_empty_password(existing_token.is_some())
        .interact()?;
    let token = if token.is_empty() {
        existing_token.unwrap_or_default().to_string()
    } else {
        token
    };
    if token.is_empty() {
        return Err(Error::new("Cannot continue without an access token."));
    }
    Ok(token)
}

fn validate_branch_prefix(branch_prefix: &str) -> Result<()> {
    // They can include slash / for hierarchical (directory) grouping, but no slash-separated component can begin with a dot . or end with the sequence .lock.
    if branch_prefix.contains("/.")
        || branch_prefix.contains(".lock/")
        || branch_prefix.ends_with(".lock")
        || branch_prefix.starts_with('.')
    {
        return Err(Error::new("Branch prefix cannot have slash-separated component beginning with a dot . or ending with the sequence .lock"));
    }

    if branch_prefix.contains("..") {
        return Err(Error::new(
            "Branch prefix cannot contain two consecutive dots anywhere.",
        ));
    }

    if branch_prefix.chars().any(|c| c.is_ascii_control()) {
        return Err(Error::new(
            "Branch prefix cannot contain ASCII control sequence",
        ));
    }

    let forbidden_chars_re = regex!(r"[ \~\^:?*\[\\]");
    if forbidden_chars_re.is_match(branch_prefix) {
        return Err(Error::new(
            "Branch prefix contains one or more forbidden characters.",
        ));
    }

    if branch_prefix.contains("//") || branch_prefix.starts_with('/') {
        return Err(Error::new("Branch prefix contains multiple consecutive slashes or starts with slash."));
    }

    if branch_prefix.contains("@{") {
        return Err(Error::new("Branch prefix cannot contain the sequence @{"));
    }

    Ok(())
}

#[derive(Debug)]
struct AuthScopes {
    scopes: Vec<String>,
}

impl FromResponse for AuthScopes {
    fn from_response<'async_trait, B>(
        response: http::Response<B>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = octocrab::Result<Self>>
                + std::marker::Send
                + 'async_trait,
        >,
    >
    where
        B: http_body::Body<Data = bytes::Bytes, Error = octocrab::Error> + Send,
        B: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let scopes = response
                .headers()
                .get("x-oauth-scopes")
                .map(|v| v.to_str())
                .transpose()
                .map_err(|err| octocrab::Error::Other {
                    source: Box::new(err),
                    backtrace: std::backtrace::Backtrace::capture(),
                })?
                .map(|value| {
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|x| !x.is_empty())
                        .map(String::from)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Ok(AuthScopes { scopes })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        github_remote_details, normalize_github_host, validate_branch_prefix,
        validate_github_host,
    };

    #[test]
    fn test_github_host() {
        assert_eq!(
            normalize_github_host("https://github.example.com/"),
            "github.example.com"
        );
        assert!(validate_github_host("github.example.com").is_ok());
        assert!(validate_github_host("github.example.com:8443").is_ok());
        assert!(validate_github_host("github.example.com/path").is_err());
    }

    #[test]
    fn test_github_remote_details() {
        for url in [
            "https://github.example.com/acme/codez.git",
            "git@github.example.com:acme/codez.git",
            "ssh://git@github.example.com/acme/codez.git",
        ] {
            assert_eq!(
                github_remote_details(url),
                Some(("github.example.com".into(), "acme/codez".into()))
            );
        }
        assert_eq!(
            github_remote_details(
                "ssh://git@github.example.com:8443/acme/codez.git"
            ),
            Some(("github.example.com:8443".into(), "acme/codez".into()))
        );
    }

    #[test]
    fn test_branch_prefix_rules() {
        // Rules taken from https://git-scm.com/docs/git-check-ref-format
        // Note: Some rules don't need to be checked because the prefix is
        // always embedded into a larger context. For example, rule 9 in the
        // reference states that a _refname_ cannot be the single character @.
        // This rule is impossible to break purely via the branch prefix.
        let bad_prefixes: Vec<(&str, &str)> = vec![
            (
                "spr/.bad",
                "Cannot start slash-separated component with dot",
            ),
            (".bad", "Cannot start slash-separated component with dot"),
            ("spr/bad.lock", "Cannot end with .lock"),
            (
                "spr/bad.lock/some_more",
                "Cannot end slash-separated component with .lock",
            ),
            (
                "spr/b..ad/bla",
                "They cannot contain two consecutive dots anywhere",
            ),
            ("spr/bad//bla", "They cannot contain consecutive slashes"),
            ("/bad", "Prefix should not start with slash"),
            ("/bad@{stuff", "Prefix cannot contain sequence @{"),
        ];

        for (branch_prefix, reason) in bad_prefixes {
            assert!(
                validate_branch_prefix(branch_prefix).is_err(),
                "{}",
                reason
            );
        }

        let ok_prefix = "spr/some.lockprefix/with-stuff/foo";
        assert!(validate_branch_prefix(ok_prefix).is_ok());
    }

    #[test]
    fn test_branch_prefix_rejects_forbidden_characters() {
        // Here I'm mostly concerned about escaping / not escaping in the regex :p
        assert!(validate_branch_prefix("bad\x1F").is_err());
        assert!(validate_branch_prefix("notbad!").is_ok());
        assert!(
            validate_branch_prefix("bad /space").is_err(),
            "Reject space in prefix"
        );
        assert!(validate_branch_prefix("bad~").is_err(), "Reject tilde");
        assert!(validate_branch_prefix("bad^").is_err(), "Reject caret");
        assert!(validate_branch_prefix("bad:").is_err(), "Reject colon");
        assert!(validate_branch_prefix("bad?").is_err(), "Reject ?");
        assert!(validate_branch_prefix("bad*").is_err(), "Reject *");
        assert!(validate_branch_prefix("bad[").is_err(), "Reject [");
        assert!(validate_branch_prefix(r"bad\").is_err(), "Reject \\");
    }
}
