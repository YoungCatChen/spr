/*
 * Copyright (c) Radical HQ Limited
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use std::sync::Arc;

use http::{header::USER_AGENT, HeaderName, HeaderValue, Uri};
use hyper_tls::HttpsConnector;
use hyper_util::{
    client::{legacy::connect::proxy::Tunnel, proxy::matcher::Matcher},
    rt::TokioExecutor,
};
use octocrab::{
    service::middleware::{
        auth_header::AuthHeaderLayer, base_uri::BaseUriLayer,
        extra_headers::ExtraHeadersLayer,
    },
    AuthState, Octocrab, OctocrabBuilder,
};

use crate::error::Result;

const GITHUB_API_URI: &str = "https://api.github.com";
const GITHUB_UPLOAD_URI: &str = "https://uploads.github.com";

pub(crate) fn new_octocrab(
    base_uri: Option<&str>,
    auth_token: Option<&str>,
    extra_headers: &[(HeaderName, String)],
) -> Result<Octocrab> {
    let api_uri: Uri = base_uri.unwrap_or(GITHUB_API_URI).parse()?;
    // Octocrab's default Hyper connector does not read proxy variables.
    let Some(proxy) = Matcher::from_env().intercept(&api_uri) else {
        let mut builder = OctocrabBuilder::new();
        if let Some(auth_token) = auth_token {
            builder = builder.personal_token(auth_token.to_string());
        }
        if let Some(base_uri) = base_uri {
            builder = builder.base_uri(base_uri)?;
        }
        for (name, value) in extra_headers {
            builder = builder.add_header(name.clone(), value.clone());
        }
        return Ok(builder.build()?);
    };

    let mut tunnel = Tunnel::new(proxy.uri().clone(), HttpsConnector::new());
    if let Some(proxy_auth) = proxy.basic_auth() {
        tunnel = tunnel.with_auth(proxy_auth.clone());
    }
    let connector = HttpsConnector::new_with_connector(tunnel);
    let client =
        hyper_util::client::legacy::Client::builder(TokioExecutor::new())
            .build(connector);

    let mut headers = vec![(USER_AGENT, HeaderValue::from_static("octocrab"))];
    headers.extend(
        extra_headers
            .iter()
            .map(|(name, value)| Ok((name.clone(), value.parse()?)))
            .collect::<Result<Vec<_>>>()?,
    );
    let auth_header = auth_token
        .map(|token| format!("Bearer {token}").parse())
        .transpose()?;
    let upload_uri = GITHUB_UPLOAD_URI.parse()?;

    Ok(OctocrabBuilder::new_empty()
        .with_service(client)
        .with_layer(&ExtraHeadersLayer::new(Arc::new(headers)))
        .with_layer(&BaseUriLayer::new(api_uri.clone()))
        .with_layer(&AuthHeaderLayer::new(auth_header, api_uri, upload_uri))
        .with_auth(AuthState::None)
        .build()?)
}
