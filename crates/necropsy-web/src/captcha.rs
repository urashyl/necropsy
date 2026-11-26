// SPDX-License-Identifier: AGPL-3.0-or-later
//! Cloudflare Turnstile, the captcha in front of account creation and of
//! starting a live instance. See `docs/accounts.md`.
//!
//! Set `NECROPSY_TURNSTILE_SITE_KEY` and `NECROPSY_TURNSTILE_SECRET` to turn it
//! on. Without them no captcha is asked for, which is fine for local runs but
//! leaves registration and instances open to scripts. A check that cannot reach
//! Cloudflare fails closed.

use std::time::Duration;

const VERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

/// Longest token Turnstile issues is well under this.
const MAX_TOKEN: usize = 2048;

pub enum Captcha {
    Off,
    Turnstile {
        site_key: String,
        secret: String,
        client: reqwest::Client,
    },
    /// Accepts the token `pass`. Tests only.
    #[cfg(test)]
    Fake,
}

impl Captcha {
    pub fn turnstile(site_key: String, secret: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("cannot build the captcha http client");
        Captcha::Turnstile {
            site_key,
            secret,
            client,
        }
    }

    /// The public site key for the widget, or `None` when no captcha is asked.
    pub fn site_key(&self) -> Option<&str> {
        match self {
            Captcha::Off => None,
            Captcha::Turnstile { site_key, .. } => Some(site_key),
            #[cfg(test)]
            Captcha::Fake => Some("test-site-key"),
        }
    }

    /// Whether the request may go ahead. Always true when no captcha is set up.
    pub async fn verify(&self, token: &str, ip: &str) -> bool {
        match self {
            Captcha::Off => true,
            #[cfg(test)]
            Captcha::Fake => token == "pass",
            Captcha::Turnstile { secret, client, .. } => {
                if token.is_empty() || token.len() > MAX_TOKEN {
                    return false;
                }
                let mut form = vec![("secret", secret.as_str()), ("response", token)];
                if ip.parse::<std::net::IpAddr>().is_ok() {
                    form.push(("remoteip", ip));
                }
                let res = client.post(VERIFY_URL).form(&form).send().await;
                match res {
                    Ok(r) => match r.json::<serde_json::Value>().await {
                        Ok(v) => v["success"].as_bool() == Some(true),
                        Err(_) => false,
                    },
                    Err(e) => {
                        tracing::warn!("captcha check could not reach Cloudflare: {e}");
                        false
                    }
                }
            }
        }
    }
}
