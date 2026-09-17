//! Account-page appearance: `?theme=light` or `?theme=dark` overrides a
//! session cookie, with light as the default. The cookie survives the Google
//! round trip and form retries; it is a preference, never an auth credential.

use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::Response,
};

const COOKIE_NAME: &str = "passband_theme";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Theme {
    #[default]
    Light,
    Dark,
}

impl Theme {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

tokio::task_local! {
    // Scoped to the request future, not a thread: concurrent account flows
    // cannot share appearance. Page rendering stays synchronous and the same
    // scope covers errors returned by rate limits or OAuth handlers.
    static THEME: Theme;
}

pub(crate) fn current() -> Theme {
    THEME.try_with(|theme| *theme).unwrap_or_default()
}

fn selection(query: Option<&str>, headers: &HeaderMap) -> (Theme, Option<Theme>) {
    let explicit = url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .find(|(key, _)| key == "theme")
        .and_then(|(_, value)| Theme::parse(&value));
    let saved = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .and_then(|(_, value)| Theme::parse(value));
    (explicit.or(saved).unwrap_or_default(), explicit)
}

pub(crate) async fn apply(State(secure): State<bool>, request: Request, next: Next) -> Response {
    let (theme, explicit) = selection(request.uri().query(), request.headers());
    let mut response = THEME.scope(theme, next.run(request)).await;
    if let Some(theme) = explicit {
        let secure = if secure { "; Secure" } else { "" };
        let cookie = format!(
            "{COOKIE_NAME}={}; Path=/; HttpOnly; SameSite=Lax{secure}",
            theme.name(),
        );
        // Append so the OAuth session cookie is preserved on redirects.
        response.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_str(&cookie).expect("theme cookie contains only fixed literals"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::StatusCode,
        middleware,
        routing::{get, post},
    };
    use tower::ServiceExt;

    fn app(secure: bool) -> Router {
        Router::new()
            .route(
                "/",
                get(|| async { crate::pages::signup_form("passband.email", None, "", "", None) }),
            )
            .route(
                "/signup",
                post(|| async {
                    crate::pages::signup_form(
                        "passband.email",
                        None,
                        "alex",
                        "INVALID",
                        Some("Check your invite code."),
                    )
                }),
            )
            .route(
                "/redirect",
                get(|| async {
                    (
                        StatusCode::SEE_OTHER,
                        [
                            (header::LOCATION, "/callback"),
                            (header::SET_COOKIE, "passband_signup=example; HttpOnly"),
                        ],
                    )
                }),
            )
            .route(
                "/callback",
                get(|| async {
                    crate::pages::app_signed_in(
                        "alex@example.com",
                        "https://alex.passband.email",
                        "DEMO-CODE",
                        10,
                        None,
                    )
                }),
            )
            .layer(middleware::from_fn_with_state(secure, apply))
    }

    #[tokio::test]
    async fn query_overrides_cookie_and_invalid_values_are_not_reflected() {
        for (query, cookie, expected) in [
            ("", "", "light"),
            ("?theme=dark", "", "dark"),
            ("?theme=light", "passband_theme=dark", "light"),
            ("?theme=dark", "passband_theme=light", "dark"),
            ("", "passband_theme=dark", "dark"),
            ("?theme=invalid", "passband_theme=dark", "dark"),
            (
                "?theme=%22%3E%3Cscript%3E",
                "passband_theme=invalid",
                "light",
            ),
        ] {
            let response = app(false)
                .oneshot(
                    Request::builder()
                        .uri(format!("/{query}"))
                        .header(header::COOKIE, cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let html = String::from_utf8(body.to_vec()).unwrap();
            assert!(
                html.contains(&format!("data-theme=\"{expected}\"")),
                "{query}"
            );
            assert!(!html.contains("<script>"));
        }
    }

    #[tokio::test]
    async fn preference_survives_redirect_without_replacing_auth_cookie() {
        let response = app(true)
            .oneshot(
                Request::builder()
                    .uri("/redirect?theme=dark")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let cookies: Vec<_> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect();
        assert_eq!(cookies.len(), 2);
        assert!(cookies.contains(&"passband_signup=example; HttpOnly"));
        assert!(cookies.contains(&"passband_theme=dark; Path=/; HttpOnly; SameSite=Lax; Secure"));
        let response = app(true)
            .oneshot(
                Request::builder()
                    .uri("/callback")
                    .header(header::COOKIE, "passband_theme=dark")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(
            String::from_utf8(body.to_vec())
                .unwrap()
                .contains("data-theme=\"dark\"")
        );
    }

    #[tokio::test]
    async fn form_errors_keep_the_theme_and_security_headers() {
        let response = app(false)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/signup")
                    .header(header::COOKIE, "passband_theme=dark")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "no-store, no-cache"
        );
        assert!(
            response.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("default-src 'none'")
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("data-theme=\"dark\""));
        assert!(html.contains("Check your invite code."));
        assert!(html.contains("value=\"alex\""));
    }

    #[tokio::test]
    async fn theme_scopes_are_isolated_and_restore_the_default() {
        let render = |theme| {
            THEME.scope(theme, async {
                tokio::task::yield_now().await;
                current()
            })
        };
        let (light, dark) = tokio::join!(render(Theme::Light), render(Theme::Dark));
        assert_eq!(light, Theme::Light);
        assert_eq!(dark, Theme::Dark);
        assert_eq!(current(), Theme::Light);
    }
}
