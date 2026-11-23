//! Card types people bring: the Settings page that takes the .zip, the
//! form for a card of such a type, and the sandboxed frame it draws in.
//! What a package may hold, and why it stays harmless, is in `types`.

use std::collections::BTreeMap;

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;
use crate::types::{CardType, FieldKind, MAX_PACKAGE_BYTES, Manifest, TypedCard};

/// The frame's own rules, on top of its `sandbox` attribute. Scripts and
/// styles come only from its package (and the bootstrap); it may connect
/// nowhere, submit nothing and sit only inside this site's pages.
const FRAME_CSP: &str = "sandbox allow-scripts; default-src 'none'; script-src 'self'; \
style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self'; connect-src 'none'; \
base-uri 'none'; form-action 'none'; frame-ancestors 'self'";

/// A package's files: never a page, even when opened on their own.
const FILE_CSP: &str = "sandbox; default-src 'none'";

pub(crate) const FRAME_JS: &str = include_str!("../../static/frame.js");

/// Where to read how a package is made.
const GUIDE: &str = "https://github.com/vdTwer/AiNKI/blob/main/docs/card-types.md";

pub(crate) async fn types_page(State(app): State<AppState>, user: WebUser, Flash(msg): Flash) -> WebResult {
    let types = app.card_types(&user.access()).await?;
    Ok(types_view(&user.0, &types, msg.as_deref(), None).into_response())
}

fn types_view(user: &User, types: &[CardType], msg: Option<&str>, error: Option<&str>) -> Markup {
    let clock = user.clock();
    layout_in(
        Nav::Settings,
        "Card types",
        Some(user),
        html! {
            (flash(msg))
            (parent("/settings", "Settings"))
            h1 { "Card types" }
            p { "A card type draws cards its own way: a chess board, a map, a keyboard. You bring it as a .zip with a "
                code { "card-type.json" } " and a JavaScript module. " a href=(GUIDE) { "How to make one" } }
            p.muted.small { "Its code runs only in your browser, in a sealed frame: it sees the card it draws and nothing else, not your other cards, not your account, not the internet. You rate the card yourself, as always." }
            p.muted.small { "To try one, " a href="/settings/types/chess.zip" download { "download the chess type" } " and upload it here." }
            (form_error(error))
            form method="post" action="/settings/types" enctype="multipart/form-data" {
                label.file.drop {
                    span.drop-text { "Choose a card type" }
                    span.muted.small { ".zip, at most 2 MB, or drop it here" }
                    input type="file" name="file" accept=".zip,application/zip" required;
                }
                div.form-foot.end { button.btn.primary type="submit" { "Upload" } }
            }
            @if !types.is_empty() {
                h2 { "Yours" }
                ul.list.type-list {
                    @for (i, t) in types.iter().enumerate() {
                        @let newest = i == 0 || types.get(i - 1).is_none_or(|p| p.name != t.name);
                        li {
                            div.grow {
                                span { (t.name) " " span.muted { "version " (t.version) } }
                                span.sub.muted.small {
                                    (count(t.cards, "card")) " · " ((t.bytes + 1023) / 1024) " KB · added " (format::when(&clock, t.created_at))
                                    @if newest { " · new cards use it" }
                                }
                                @if newest && !t.manifest.description.is_empty() { span.sub.small { (t.manifest.description) } }
                            }
                            @if newest { a href=(format!("/cards/new?type={}", t.id)) { "Add card" } }
                            @if t.cards == 0 {
                                form method="post" action=(format!("/settings/types/{}/delete", t.id)) {
                                    button.link type="submit" { "Delete" }
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}

pub(crate) async fn type_upload(State(app): State<AppState>, user: WebUser, mp: Multipart) -> WebResult {
    let access = user.access();
    let result = async {
        let file = read_multipart(mp).await?.remove("file").filter(|d| !d.is_empty());
        let file = file.ok_or_else(|| AppError::bad("choose a .zip file"))?;
        app.upload_card_type(&access, &file).await
    }
    .await;
    match result {
        Ok(t) => Ok(redirect_msg("/settings/types", &format!("{} version {} is ready.", t.name, t.version))),
        Err(e) => {
            let msg = fixable(&e).ok_or(e)?;
            let types = app.card_types(&access).await?;
            Ok(form_again(types_view(&user.0, &types, None, Some(&msg))))
        }
    }
}

pub(crate) async fn type_delete(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
    or_back(
        "/settings/types",
        async {
            app.delete_card_type(&user.access(), id).await?;
            Ok(redirect_msg("/settings/types", "Deleted."))
        }
        .await,
    )
}

/// The chess type, the first one brought, as a .zip to download and upload:
/// an example of a package, and a way to try one.
const CHESS: [(&str, &[u8]); 6] = [
    ("card-type.json", include_bytes!("../../../packages/chess/card-type.json")),
    ("chess.js", include_bytes!("../../../packages/chess/chess.js")),
    ("chess.css", include_bytes!("../../../packages/chess/chess.css")),
    ("board.js", include_bytes!("../../../packages/chess/board.js")),
    ("board.css", include_bytes!("../../../packages/chess/board.css")),
    ("sprites.js", include_bytes!("../../../packages/chess/sprites.js")),
];

pub(crate) fn chess_zip() -> Result<Vec<u8>, AppError> {
    use std::io::Write;
    let fail = |e: &dyn std::fmt::Display| AppError::Internal(anyhow::anyhow!("chess.zip: {e}"));
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, body) in CHESS {
        w.start_file(name, opts).map_err(|e| fail(&e))?;
        w.write_all(body).map_err(|e| fail(&e))?;
    }
    Ok(w.finish().map_err(|e| fail(&e))?.into_inner())
}

pub(crate) async fn chess_download(_user: WebUser) -> WebResult {
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip"),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"chess.zip\""),
        ],
        chess_zip()?,
    )
        .into_response())
}

/// The body limit for an upload: the package and the form around it.
pub(crate) const UPLOAD_LIMIT: usize = MAX_PACKAGE_BYTES + 64 * 1024;

// ----- the frame ------------------------------------------------------------

fn with_headers(mut res: Response, csp: &'static str, cache: &'static str) -> Response {
    let h = res.headers_mut();
    h.insert("content-security-policy", HeaderValue::from_static(csp));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    // The frame's origin is its own (opaque), so its requests for these
    // files are cross-origin and carry no cookies.
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    res
}

/// The page inside the sandboxed frame: the package's styles, then the
/// bootstrap, which loads the module and talks to the study page.
pub(crate) async fn type_frame(State(app): State<AppState>, Path(token): Path<String>) -> WebResult {
    let m = app.card_type_by_token(&token).await?;
    let page = html! {
        (maud::DOCTYPE)
        html {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                style { (maud::PreEscaped(FRAME_STYLE)) }
                @for s in &m.styles { link rel="stylesheet" href=(s); }
                script type="module" src=(format!("/types/frame.js?v={}", asset_version())) {}
            }
            body data-module=(m.module) { main id="card" {} }
        }
    };
    let mut res = with_headers(page.into_response(), FRAME_CSP, "no-cache");
    res.headers_mut().insert("x-frame-options", HeaderValue::from_static("SAMEORIGIN"));
    Ok(res)
}

/// The frame's defaults: the study page's font and colors (the colors come
/// with each card), and no background of its own.
const FRAME_STYLE: &str = ":root { color-scheme: light dark; font: calc(17px * var(--scale, 1)) / 1.55 system-ui, sans-serif; \
color: var(--ink, CanvasText); } html, body { margin: 0; background: transparent; } body { display: flow-root; overflow: hidden; } \
.type-failed { color: var(--muted, GrayText); font-size: .875rem; }";

pub(crate) async fn type_file(
    State(app): State<AppState>,
    Path((token, path)): Path<(String, String)>,
) -> WebResult {
    let (mime, body) = app.card_type_file(&token, &path).await?;
    let mut res = body.into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).map_err(|e| AppError::Internal(e.into()))?);
    // A version never changes: a new upload gets a new address.
    Ok(with_headers(res, FILE_CSP, "public, max-age=31536000, immutable"))
}

pub(crate) async fn frame_js(q: RawQuery) -> Response {
    let mut res = asset(q, "text/javascript; charset=utf-8", FRAME_JS);
    res.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    res
}

/// A card's frame: the challenge, or the solution once turned over.
/// `ainki-card-type` (parts.js) hands it the fields.
pub(crate) fn type_frame_box(ty: &CardType, fields_json: &str, side: &str, preview: bool) -> Markup {
    html! {
        ainki-card-type data-fields=(fields_json) data-side=(side) data-preview[preview] {
            iframe.type-frame sandbox="allow-scripts" src=(format!("/types/{}/frame", ty.token)) title=(ty.name) {}
            @if preview {
                p.type-sides.small {
                    button.link type="button" data-side="challenge" aria-pressed="true" { "Front" }
                    " · "
                    button.link type="button" data-side="solution" aria-pressed="false" { "Back" }
                }
            }
        }
    }
}

/// A typed card on the study page: its frame, with the plain summary for
/// browsers without JavaScript.
pub(crate) fn typed_faces(card: &Card, typed: &TypedCard, revealed: bool) -> Markup {
    html! {
        div.card-area.typed {
            (state_tag(card.card_state()))
            (type_frame_box(&typed.ty, &typed.fields_json(), if revealed { "solution" } else { "challenge" }, false))
            noscript { section.face.front { (face(&card.front, None)) } }
            div id="answer" {}
        }
    }
}

// ----- writing a card of a brought type ---------------------------------------

/// The deck picker of the card forms.
pub(crate) fn deck_line(decks: &[Deck], deck: Option<i64>, new_deck: &str, allow_new_deck: bool) -> Markup {
    let new_selected = allow_new_deck && (decks.is_empty() || deck.is_none());
    html! {
        div.deck-line {
            label.deck-pick { span.deck-label { "Deck" }
                span.select {
                    select name="deck" required data-new-deck-select=(NEW_DECK) {
                        @for d in decks { option value=(d.id) selected[!new_selected && Some(d.id) == deck] { (deck_plain(&d.name)) } }
                        @if allow_new_deck {
                            option value=(NEW_DECK) selected[new_selected] { "New deck…" }
                        }
                    }
                }
            }
            @if allow_new_deck {
                label.new-deck id="new-deck-field" {
                    span.deck-label { "New deck name" }
                    input type="text" name="new_deck" maxlength="200" placeholder="e.g. Spanish verbs" value=(new_deck);
                }
            }
        }
    }
}

/// The form for a card of a brought type: one field each, and the card as
/// the type draws it.
pub(crate) struct TypedDraft<'a> {
    pub(crate) action: String,
    pub(crate) deck: Option<i64>,
    pub(crate) new_deck: &'a str,
    pub(crate) ty: &'a CardType,
    pub(crate) values: &'a BTreeMap<String, String>,
    pub(crate) submit: &'a str,
    pub(crate) allow_new_deck: bool,
}

pub(crate) fn typed_form(decks: &[Deck], d: &TypedDraft) -> Markup {
    let m: &Manifest = &d.ty.manifest;
    let fields = serde_json::to_string(d.values).unwrap_or_else(|_| "{}".into());
    html! {
        form.type-form method="post" action=(d.action) {
            input type="hidden" name="type" value=(d.ty.id);
            (deck_line(decks, d.deck, d.new_deck, d.allow_new_deck))
            div.sheet {
                @for (i, f) in m.fields.iter().enumerate() {
                    @let id = format!("field-{}", f.key);
                    @let name = format!("f.{}", f.key);
                    @let value = d.values.get(&f.key).map_or("", String::as_str);
                    div.face-edit {
                        label.face-label for=(id) { (f.label) @if f.optional { " " span.muted.optional { "(optional)" } } }
                        @match f.kind {
                            FieldKind::Line => input id=(id) name=(name) type="text" value=(value) required[!f.optional] autofocus[i == 0] autocomplete="off" spellcheck="false";
                            FieldKind::Text => textarea id=(id) name=(name) rows="3" required[!f.optional] { (value) }
                        }
                        @if !f.hint.is_empty() { p.hint.muted.small { (f.hint) } }
                    }
                }
            }
            div.sheet.type-preview { (type_frame_box(d.ty, &fields, "challenge", true)) }
            div.form-foot.end {
                button.btn.primary.wide type="submit" { (d.submit) span.key.submit-key hidden { "Ctrl ⏎" } }
            }
        }
    }
}

/// The fields sent with a typed card's form: `f.<key>` each.
fn sent_fields(form: &[(String, String)]) -> BTreeMap<String, String> {
    form.iter().filter_map(|(k, v)| k.strip_prefix("f.").map(|k| (k.to_string(), v.clone()))).collect()
}

fn sent(form: &[(String, String)], key: &str) -> Option<String> {
    form.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

/// The types a new card can have, linked above the Add card form.
pub(crate) fn type_pick(types: &[CardType], current: Option<i64>, deck: Option<i64>) -> Markup {
    let deck = deck.map_or_else(String::new, |d| format!("deck={d}&"));
    html! {
        @if !types.is_empty() {
            nav.type-pick aria-label="Card type" {
                a href=(format!("/cards/new?{}", deck.trim_end_matches('&'))) aria-current=[current.is_none().then_some("page")] { "Plain" }
                @for t in types {
                    a href=(format!("/cards/new?{deck}type={}", t.id)) aria-current=[(current == Some(t.id)).then_some("page")] { (t.name) }
                }
            }
        }
    }
}

pub(crate) async fn typed_new_page(
    app: &App,
    user: &User,
    type_id: i64,
    deck: Option<i64>,
    msg: Option<&str>,
) -> Result<Markup, AppError> {
    let access = Access::web(user.clone());
    let ty = app.card_type(&access, type_id).await?;
    let decks = open_decks(app.decks(&access).await?, deck);
    let deck = match deck {
        Some(id) => Some(id),
        None => app.last_used_deck(&access).await?.or_else(|| decks.first().map(|d| d.id)),
    };
    let types = app.latest_card_types(&access).await?;
    let values = BTreeMap::new();
    let draft = TypedDraft {
        action: "/cards/new/typed".into(),
        deck,
        new_deck: "",
        ty: &ty,
        values: &values,
        submit: "Add card",
        allow_new_deck: true,
    };
    Ok(typed_new_view(user, &decks, &types, &draft, msg, None))
}

fn typed_new_view(
    user: &User,
    decks: &[Deck],
    types: &[CardType],
    d: &TypedDraft,
    msg: Option<&str>,
    error: Option<&str>,
) -> Markup {
    layout_in(
        Nav::Add,
        "Add card",
        Some(user),
        html! {
            (add_head("card", "Add card", flash(msg)))
            (type_pick(types, Some(d.ty.id), d.deck))
            (form_error(error))
            (typed_form(decks, d))
        },
    )
}

pub(crate) async fn typed_create(
    State(app): State<AppState>,
    user: WebUser,
    Form(form): Form<Vec<(String, String)>>,
) -> WebResult {
    let access = user.access();
    let fields = sent_fields(&form);
    let type_id: i64 = sent(&form, "type").and_then(|t| t.parse().ok()).ok_or(AppError::NotFound)?;
    let deck_sent = sent(&form, "deck").unwrap_or_default();
    let new_deck = sent(&form, "new_deck").filter(|_| deck_sent == NEW_DECK);
    let result = async {
        let (deck, msg) = match &new_deck {
            Some(name) => deck_for_name(&app, &access, name).await?,
            None => (deck_sent.parse().map_err(|_| AppError::bad("pick a deck"))?, "Card added.".to_string()),
        };
        app.create_typed_card(&access, deck, type_id, &fields).await?;
        Ok::<_, AppError>((deck, msg))
    }
    .await;
    match result {
        Ok((deck, msg)) => Ok(redirect_msg(format!("/cards/new?deck={deck}&type={type_id}"), &msg)),
        Err(e) => {
            let msg = fixable(&e).ok_or(e)?;
            let ty = app.card_type(&access, type_id).await?;
            let deck = deck_sent.parse().ok();
            let decks = open_decks(app.decks(&access).await?, deck);
            let types = app.latest_card_types(&access).await?;
            let draft = TypedDraft {
                action: "/cards/new/typed".into(),
                deck,
                new_deck: new_deck.as_deref().unwrap_or(""),
                ty: &ty,
                values: &fields,
                submit: "Add card",
                allow_new_deck: true,
            };
            Ok(form_again(typed_new_view(&user.0, &decks, &types, &draft, None, Some(&msg))))
        }
    }
}

/// A new deck by name, or the one already called that; with what to say.
pub(crate) async fn deck_for_name(app: &App, access: &Access, name: &str) -> Result<(i64, String), AppError> {
    if name.trim().is_empty() {
        return Err(AppError::bad("give the new deck a name"));
    }
    Ok(if let Some(existing) = app.deck_by_name(access, name).await? {
        (existing.id, format!("Card added to your existing deck {}.", deck_plain(&existing.name)))
    } else {
        (app.create_deck(access, name, "", None).await?.id, "Deck created and card added.".into())
    })
}

pub(crate) async fn typed_update(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Query(from): Query<CardFrom>,
    Form(form): Form<Vec<(String, String)>>,
) -> WebResult {
    let access = user.access();
    let fields = sent_fields(&form);
    let result = async {
        app.update_typed_card(&access, id, &fields).await?;
        if let Some(deck) = sent(&form, "deck").and_then(|d| d.parse().ok())
            && deck != app.card(&access, id).await?.deck_id
        {
            app.move_card(&access, id, deck).await?;
        }
        Ok::<_, AppError>(())
    }
    .await;
    match result {
        Ok(()) => Ok(redirect_msg(from.study_url().unwrap_or_else(|| format!("/cards/{id}")), "Card saved.")),
        Err(e) => {
            let msg = fixable(&e).ok_or(e)?;
            let sent = TypedSent { deck: sent(&form, "deck").and_then(|d| d.parse().ok()), fields };
            Ok(form_again(card_view(&app, &user.0, id, &from, None, Sent::Typed(&sent), Some(&msg)).await?))
        }
    }
}

/// What came back from a typed card's edit form with a problem.
pub(crate) struct TypedSent {
    pub(crate) deck: Option<i64>,
    pub(crate) fields: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_chess_type_is_a_valid_package() {
        let p = crate::types::read_package(&super::chess_zip().unwrap()).unwrap();
        assert_eq!(p.manifest.name, "Chess");
        assert_eq!(p.files.len(), 5);
    }
}
