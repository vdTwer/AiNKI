//! The Access page: API tokens and what each may do.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

// What an agent may do, worded once for the token form and the OAuth
// consent page.
const PERM_WRITE: &str = "Add, edit and delete cards and decks";
const PERM_REVIEW: &str = "Quiz you and record your answers";
const PERM_ALL_DECKS: &str = "All decks (also lets it create decks, change presets and share)";
const PERM_SOME_DECKS: &str = "Only these:";

pub(crate) fn mcp_url(app: &AppState, headers: &HeaderMap) -> String {
    format!("{}/mcp", oauth::base_url(app, headers))
}

pub(crate) async fn access_page(
    State(app): State<AppState>,
    user: WebUser,
    headers: HeaderMap,
    Flash(msg): Flash,
) -> WebResult {
    let access = user.access();
    let tokens = app.list_tokens(user.0.id).await?;
    let decks = app.decks(&access).await?;
    let clock = user.0.clock();
    let url = mcp_url(&app, &headers);
    Ok(layout_in(
        Nav::Settings,
        "Agent access",
        Some(&user.0),
        html! {
            (flash(msg.as_deref()))
            (parent("/settings", "Settings"))
            h1 { "Agent access" }
            (connect_steps(&url))
            (extension_offer())
            (connected_tokens(&tokens, &clock))
            (token_form(&decks))
        },
    )
    .into_response())
}

fn connect_steps(url: &str) -> Markup {
    let https = url.starts_with("https://");
    html! {
        h2 { "Connect Claude" }
        ol.prose {
            li { "In Claude, open " strong { "Settings → Connectors" } " and choose " strong { "Add custom connector" } "." }
            li { "Name it AiNKI and paste this URL:" pre.copy { (url) } }
            li { "Click " strong { "Connect" } ", log in here if asked, and click " strong { "Allow" } "." }
        }
        @if !https {
            p.notice {
                "This address isn't HTTPS. Claude on the web and in the apps needs AiNKI on a public HTTPS address; "
                "set AINKI_PUBLIC_URL to it. Claude Code on this computer can use it as is: "
                code { "claude mcp add --transport http ainki " (url) }
            }
        }
        p.muted.small { "Any MCP client that supports OAuth connects the same way. Agents can never change your password or settings." }
    }
}

/// The signed Firefox extension: each version bump in the extension's
/// repository publishes a GitHub release with this file.
const EXTENSION_URL: &str = "https://github.com/vdTwer/AiNKI-Extension/releases/latest/download/ainki.xpi";

fn extension_offer() -> Markup {
    html! {
        h2 { "Firefox extension" }
        p { "Turn selected text and images on any page into cards. Firefox asks before it installs; then connect it with your AiNKI address." }
        p { a.btn.primary href=(EXTENSION_URL) { "Get the extension" } }
    }
}

fn connected_tokens(tokens: &[(crate::auth::ApiToken, Vec<String>)], clock: &scheduler::Clock) -> Markup {
    html! {
        h2 { "Connected" }
        @if tokens.is_empty() { p.muted { "Nothing connected yet." } }
        ul.list hidden[tokens.is_empty()] {
            @for (t, deck_names) in tokens {
                li.item {
                    div.grow {
                        strong { (t.name) } " "
                        @if t.oauth_client_id.is_some() { span.small { "(connected app)" } } @else { code { (t.prefix) "…" } }
                        br;
                        span.small {
                            (if t.can_write { "read & write" } else { "read only" })
                            (if t.can_review { " · can review" } else { "" })
                            " · "
                            @if t.all_decks { "all decks" } @else { (deck_names.iter().map(|n| deck_plain(n)).collect::<Vec<_>>().join(", ")) }
                        }
                        br;
                        span.muted.small {
                            "Last used "
                            (t.last_used_at.map_or_else(|| "never".into(), |ts| format::when(clock, ts)))
                        }
                    }
                    form method="post" action=(format!("/access/{}/revoke", t.id)) {
                        button.btn.danger type="submit" {
                            (if t.oauth_client_id.is_some() { "Disconnect" } else { "Revoke" })
                        }
                    }
                }
            }
        }
    }
}

/// What an agent may do, the same on the token form and the OAuth consent
/// page: write, review, and all decks or some.
pub(crate) fn permission_fields(decks: &[Deck]) -> Markup {
    html! {
        label.check { input type="checkbox" name="can_write" value="1" checked; " " (PERM_WRITE) }
        label.check { input type="checkbox" name="can_review" value="1" checked; " " (PERM_REVIEW) }
        fieldset {
            legend { "Decks" }
            label.check { input type="radio" name="deck_scope" value="all" checked; " " (PERM_ALL_DECKS) }
            @if !decks.is_empty() {
                label.check { input type="radio" name="deck_scope" value="some"; " " (PERM_SOME_DECKS) }
                @for d in decks {
                    label.check.indent { input type="checkbox" name="decks" value=(d.id); " " (deck_plain(&d.name)) }
                }
            }
        }
    }
}

fn token_form(decks: &[Deck]) -> Markup {
    html! {
        details {
            summary { "Create a token by hand" }
            p.small { "For agents without OAuth support: they send the token as " code { "Authorization: Bearer <token>" } "." }
            form method="post" action="/access/new" {
                label { "Name" input type="text" name="name" required placeholder="e.g. Claude"; }
                (permission_fields(decks))
                button.btn type="submit" { "Create token" }
            }
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct TokenForm {
    pub(crate) name: String,
    pub(crate) can_write: Option<String>,
    pub(crate) can_review: Option<String>,
    pub(crate) deck_scope: String,
    #[serde(default)]
    pub(crate) decks: Vec<i64>,
}

pub(crate) async fn token_create(
    State(app): State<AppState>,
    user: WebUser,
    headers: HeaderMap,
    Form(f): Form<TokenForm>,
) -> WebResult {
    let decks = if f.deck_scope == "all" { None } else { Some(f.decks) };
    let (token, raw) =
        match app.create_token(user.0.id, &f.name, f.can_write.is_some(), f.can_review.is_some(), decks).await {
            Ok(t) => t,
            Err(e) => return or_back("/access", Err(e)),
        };
    let url = mcp_url(&app, &headers);
    let config = serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "ainki": {
                "type": "http",
                "url": url,
                "headers": { "Authorization": format!("Bearer {raw}") }
            }
        }
    }))
    .unwrap_or_default();
    Ok(layout_in(
        Nav::Settings,
        "Token created",
        Some(&user.0),
        html! {
            h1 { "Token created" }
            p.notice { "Copy it now. It won't be shown again." }
            p { strong { (token.name) } }
            pre.copy { (raw) }
            h2 { "Connect an agent" }
            p { "MCP server URL:" }
            pre.copy { (url) }
            p { "Send the token as a header: " code { "Authorization: Bearer <token>" } }
            p { "Claude Code:" }
            pre.copy { "claude mcp add --transport http ainki " (url) " --header \"Authorization: Bearer " (raw) "\"" }
            p { "JSON config (Claude Desktop, Cursor and others):" }
            pre.copy { (config) }
            p { a.btn href="/access" { "Done" } }
        },
    )
    .into_response())
}

pub(crate) async fn token_revoke(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
    or_back(
        "/access",
        async {
            app.revoke_token(user.0.id, id).await?;
            Ok(redirect_msg("/access", "Access removed."))
        }
        .await,
    )
}
