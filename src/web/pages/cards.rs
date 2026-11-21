//! Adding and editing cards: the card form, image uploads, the live preview.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

#[derive(Default)]
pub(crate) struct CardFormData {
    pub(crate) deck: Option<i64>,
    /// Set when the picker says "New deck…": the name to create.
    pub(crate) new_deck: Option<String>,
    pub(crate) front: String,
    pub(crate) back: String,
    pub(crate) front_upload: Option<Vec<u8>>,
    pub(crate) back_upload: Option<Vec<u8>>,
    pub(crate) remove_front_image: bool,
    pub(crate) remove_back_image: bool,
    /// How to learn it item by item, if at all.
    pub(crate) multi: Option<Split>,
    /// An image was picked; it can't be refilled if the form comes back.
    pub(crate) had_upload: bool,
}

/// Reads a multipart form into field name → bytes. Card and import forms
/// both come through here, so upload problems read the same.
pub(crate) async fn read_multipart(mut mp: Multipart) -> Result<HashMap<String, Vec<u8>>, AppError> {
    let failed = |e: axum::extract::multipart::MultipartError| AppError::bad(format!("upload failed: {e}"));
    let mut fields = HashMap::new();
    while let Some(field) = mp.next_field().await.map_err(failed)? {
        let name = field.name().unwrap_or("").to_string();
        let data = field.bytes().await.map_err(failed)?;
        fields.insert(name, data.to_vec());
    }
    Ok(fields)
}

pub(crate) fn field_text(fields: &HashMap<String, Vec<u8>>, k: &str) -> String {
    fields.get(k).map_or_else(Default::default, |v| String::from_utf8_lossy(v).to_string())
}

pub(crate) async fn read_card_form(mp: Multipart) -> Result<CardFormData, AppError> {
    let mut fields = read_multipart(mp).await?;
    let text = |k: &str| field_text(&fields, k);
    let deck = text("deck");
    let mut f = CardFormData {
        // "New deck…" in the picker means: create the deck named alongside.
        new_deck: (deck == NEW_DECK).then(|| text("new_deck")),
        deck: deck.parse().ok(),
        front: text("front"),
        back: text("back"),
        remove_front_image: fields.contains_key("remove_front_image"),
        remove_back_image: fields.contains_key("remove_back_image"),
        // "list" is what the form sent before multicards.
        multi: Split::parse(&text("multi")).or_else(|| fields.contains_key("list").then_some(Split::List)),
        ..Default::default()
    };
    f.front_upload = fields.remove("front_image").filter(|d| !d.is_empty());
    f.back_upload = fields.remove("back_image").filter(|d| !d.is_empty());
    f.had_upload = f.front_upload.is_some() || f.back_upload.is_some();
    for d in [&f.front_upload, &f.back_upload].into_iter().flatten() {
        media::check_size(d)?;
    }
    Ok(f)
}

/// The "Add" section: one click to a new card, a new deck, or an import.
/// Three tabs over the page's title.
#[expect(clippy::needless_pass_by_value, reason = "maud renders by reference; callers build the markup inline")]
pub(crate) fn add_head(current: &str, title: &str, notice: Markup) -> Markup {
    const PLACES: [(&str, &str, &str); 3] =
        [("card", "/cards/new", "Card"), ("deck", "/decks/new", "Deck"), ("import", "/import", "Import")];
    // Merging is a kind of new deck: no tab of its own (it is linked from
    // the new-deck page).
    let tab = if current == "merge" { "deck" } else { current };
    html! {
        nav.tabs aria-label="Add" {
            @for (key, href, label) in PLACES {
                a href=(href) aria-current=[(key == tab).then_some("page")] { (label) }
            }
        }
        (notice)
        h1 { (title) }
    }
}

/// Value of the deck picker's "New deck…" option.
pub(crate) const NEW_DECK: &str = "new";

/// One side of the card being written: its text, and an optional image
/// that can be picked, dropped or pasted.
pub(crate) fn face_field(
    side: &str,
    title: &str,
    placeholder: &str,
    text: &str,
    image: Option<&str>,
    autofocus: bool,
) -> Markup {
    html! {
        div.face-edit data-face=(side) {
            label.face-label for=(side) { (title) }
            textarea id=(side) name=(side) rows="3" placeholder=(placeholder) autofocus[autofocus] { (text) }
            @if let Some(img) = image {
                div.face-image.current {
                    img.thumb src=(format!("/media/{img}")) alt=(format!("{title} image"));
                    label.check { input type="checkbox" name=(format!("remove_{side}_image")) value="1"; " Remove image" }
                }
            }
            div.face-image.preview hidden {
                img alt="";
                button.link type="button" data-clear-image { "Remove" }
            }
            label.file.attach {

                span.attach-text { @if image.is_some() { "Replace image" } @else { "Add image" } }
                " " span.muted.optional { "(optional)" }
                input type="file" name=(format!("{side}_image")) accept="image/*";
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// What the card form shows: a new card's empty sides, a card being
/// edited, or what was sent when it comes back with a problem.
pub(crate) struct CardDraft<'a> {
    /// The card being edited; none for a new one.
    pub(crate) id: Option<i64>,
    pub(crate) deck: Option<i64>,
    pub(crate) new_deck: &'a str,
    pub(crate) front: &'a str,
    pub(crate) back: &'a str,
    pub(crate) front_image: &'a Option<String>,
    pub(crate) back_image: &'a Option<String>,
    pub(crate) multi: Option<Split>,
}

/// A hint from the coach while writing, with a link when it points at
/// another card.
#[derive(serde::Serialize)]
pub(crate) struct WriteHint {
    pub(crate) text: String,
    pub(crate) href: Option<String>,
}

/// What the coach says about a card being written: a long list, or a front
/// like one you already have.
pub(crate) async fn write_hints(
    app: &App,
    access: &Access,
    front: &str,
    back: &str,
    how: Option<Split>,
    except: Option<i64>,
) -> Result<Vec<WriteHint>, AppError> {
    let mut out: Vec<WriteHint> =
        coach::write_hints(back, how).into_iter().map(|text| WriteHint { text, href: None }).collect();
    if let Some(s) = app.similar_cards(access, front, except).await?.into_iter().next() {
        out.push(WriteHint {
            text: format!("You have a card like this: “{}”. Cards this alike get mixed up.", truncate(&s.front, 60)),
            href: Some(format!("/cards/{}", s.card_id)),
        });
    }
    Ok(out)
}

pub(crate) fn hint_lines(hints: &[WriteHint]) -> Markup {
    html! {
        @for h in hints {
            p.coach-hint {
                (h.text)
                @if let Some(href) = &h.href { " " a href=(href) { "Open it" } }
            }
        }
    }
}

pub(crate) fn card_form(
    action: &str,
    decks: &[crate::service::Deck],
    c: &CardDraft,
    submit: &str,
    allow_new_deck: bool,
    hints: &[WriteHint],
) -> Markup {
    let new_selected = allow_new_deck && (decks.is_empty() || c.deck.is_none());
    html! {
        form.card-form method="post" action=(action) enctype="multipart/form-data" data-card=[c.id] {
            div.deck-line {
                label.deck-pick { span.deck-label { "Deck" }
                    span.select {
                        select name="deck" required data-new-deck-select=(NEW_DECK) {
                            @for d in decks { option value=(d.id) selected[!new_selected && Some(d.id) == c.deck] { (deck_plain(&d.name)) } }
                            @if allow_new_deck {
                                option value=(NEW_DECK) selected[new_selected] { "New deck…" }
                            }
                        }
                    }
                }
                @if allow_new_deck {
                    label.new-deck id="new-deck-field" {
                        span.deck-label { "New deck name" }
                        input type="text" name="new_deck" maxlength="200" placeholder="e.g. Spanish verbs" value=(c.new_deck);
                    }
                }
            }
            div.sheet {
                (face_field("front", "Front", "Question", c.front, c.front_image.as_deref(), true))
                hr.divider;
                (face_field("back", "Back", "Answer", c.back, c.back_image.as_deref(), false))
                // The ways the back can be split are offered; app.js keeps
                // them up to date while you write.
                @let splits = multi::splits(c.back);
                fieldset.line.multi-pick hidden[splits.is_empty() && c.multi.is_none()] {
                    legend { "Learn" }
                    span.choice {
                        label.check { input type="radio" name="multi" value="" checked[c.multi.is_none()]; " As one card" }
                        @for k in Split::ALL {
                            label.check data-split=(k.as_str()) hidden[!splits.contains(&k) && c.multi != Some(k)] {
                                input type="radio" name="multi" value=(k.as_str()) checked[c.multi == Some(k)];
                                " " (k.label())
                            }
                        }
                    }
                    p.hint-inline { "Item by item: you mark what you remembered, and each item gets its own schedule." }
                }
            }
            div.sheet.card-preview hidden {}
            div.write-hints aria-live="polite" { (hint_lines(hints)) }
            div.form-foot {
                p.muted.small {
                    "Text supports Markdown: **bold**, *italic*, lists, # headings, `code`."
                    " " button.link.preview-toggle type="button" hidden { "Preview" }
                }
                button.btn.primary.wide type="submit" { (submit) span.key.submit-key hidden { "Ctrl ⏎" } }
            }
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct NewCardQuery {
    pub(crate) deck: Option<i64>,
}

pub(crate) fn card_new_view(
    user: &User,
    decks: &[Deck],
    c: &CardDraft,
    msg: Option<&str>,
    error: Option<&str>,
) -> Markup {
    layout_in(
        Nav::Add,
        "Add card",
        Some(user),
        html! {
            (add_head("card", "Add card", flash(msg)))
            (form_error(error))
            (card_form("/cards/new", decks, c, "Add card", true, &[]))
        },
    )
}

pub(crate) async fn card_new_page(
    State(app): State<AppState>,
    user: WebUser,
    Flash(msg): Flash,
    Query(q): Query<NewCardQuery>,
) -> WebResult {
    let access = user.access();
    let decks = open_decks(app.decks(&access).await?, q.deck);
    // The deck from the link, else the one last added to, else the first.
    let deck = match q.deck {
        Some(id) => Some(id),
        None => app.last_used_deck(&access).await?.or_else(|| decks.first().map(|d| d.id)),
    };
    let draft = CardDraft {
        id: None,
        deck,
        new_deck: "",
        front: "",
        back: "",
        front_image: &None,
        back_image: &None,
        multi: None,
    };
    Ok(card_new_view(&user.0, &decks, &draft, msg.as_deref(), None).into_response())
}

/// A picked image can't be put back into a form; say so with the problem.
pub(crate) fn with_image_hint(msg: String, f: &CardFormData) -> String {
    if f.had_upload { format!("{msg} Choose the image again.") } else { msg }
}

pub(crate) async fn card_create(State(app): State<AppState>, user: WebUser, mp: Multipart) -> WebResult {
    let access = user.access();
    let mut f = read_card_form(mp).await?;
    let result = async {
        // A taken name adds to that deck instead of failing; the message says so.
        let mut msg = "Card added.".to_string();
        let deck = match (&f.new_deck, f.deck) {
            (Some(name), _) => {
                if name.trim().is_empty() {
                    return Err(AppError::bad("give the new deck a name"));
                }
                if let Some(existing) = app.deck_by_name(&access, name).await? {
                    msg = format!("Card added to your existing deck {}.", deck_plain(&existing.name));
                    existing.id
                } else {
                    msg = "Deck created and card added.".into();
                    app.create_deck(&access, name, "", None).await?.id
                }
            }
            (None, Some(id)) => id,
            (None, None) => return Err(AppError::bad("pick a deck")),
        };
        let front_image = match f.front_upload.take() {
            Some(d) => Some(app.store_image(&access, d).await?.id),
            None => None,
        };
        let back_image = match f.back_upload.take() {
            Some(d) => Some(app.store_image(&access, d).await?.id),
            None => None,
        };
        let new = NewCard { front_image, back_image, multi: f.multi, ..NewCard::new(&f.front, &f.back) };
        app.create_card(&access, deck, new).await?;
        Ok((deck, msg))
    }
    .await;
    match result {
        Ok((deck, msg)) => Ok(redirect_msg(format!("/cards/new?deck={deck}"), &msg)),
        Err(e) => {
            let msg = with_image_hint(fixable(&e).ok_or(e)?, &f);
            let decks = open_decks(app.decks(&access).await?, f.deck);
            let draft = CardDraft {
                id: None,
                deck: if f.new_deck.is_some() { None } else { f.deck },
                new_deck: f.new_deck.as_deref().unwrap_or(""),
                front: &f.front,
                back: &f.back,
                front_image: &None,
                back_image: &None,
                multi: f.multi,
            };
            Ok(form_again(card_new_view(&user.0, &decks, &draft, None, Some(&msg))))
        }
    }
}

/// Where the card page was opened from. Opened while studying, it leads
/// back to the session, and saving returns there too.
#[derive(Deserialize, Default)]
pub(crate) struct CardFrom {
    pub(crate) study: Option<u8>,
    pub(crate) deck: Option<i64>,
}

impl CardFrom {
    pub(crate) const fn study(deck: Option<i64>) -> Self {
        Self { study: Some(1), deck }
    }

    pub(crate) fn query(&self) -> String {
        match (self.study, self.deck) {
            (Some(1), Some(d)) => format!("?study=1&deck={d}"),
            (Some(1), None) => "?study=1".into(),
            _ => String::new(),
        }
    }

    pub(crate) fn study_url(&self) -> Option<String> {
        (self.study == Some(1)).then(|| study_url(self.deck))
    }
}

/// The card page; `sent` and `error` when an edit comes back with a problem.
pub(crate) async fn card_view(
    app: &App,
    user: &User,
    id: i64,
    from: &CardFrom,
    msg: Option<&str>,
    sent: Option<&CardFormData>,
    error: Option<&str>,
) -> Result<Markup, AppError> {
    let access = Access::web(user.clone());
    let card = app.card(&access, id).await?;
    let decks = open_decks(app.decks(&access).await?, Some(card.deck_id));
    let history = app.card_history(&access, id).await?;
    let items = app.card_items(&card).await?;
    let flags = app.card_flags(&access, &card).await?;
    // The coach box already names a duplicate; the form only adds the list
    // hint, unless the box says the same.
    let hints: Vec<WriteHint> = coach::write_hints(&card.back, card.multi)
        .into_iter()
        .filter(|_| !flags.iter().any(|f| f.kind == coach::Kind::Split))
        .map(|text| WriteHint { text, href: None })
        .collect();
    let clock = user.clock();
    let decay = scheduler::decay_for(&app.deck_params(&access, card.deck_id).await?)?;
    let draft = match sent {
        Some(f) => CardDraft {
            id: Some(card.id),
            deck: f.deck.or(Some(card.deck_id)),
            new_deck: "",
            front: &f.front,
            back: &f.back,
            front_image: if f.remove_front_image { &None } else { &card.front_image },
            back_image: if f.remove_back_image { &None } else { &card.back_image },
            multi: f.multi,
        },
        None => CardDraft {
            id: Some(card.id),
            deck: Some(card.deck_id),
            new_deck: "",
            front: &card.front,
            back: &card.back,
            front_image: &card.front_image,
            back_image: &card.back_image,
            multi: card.multi,
        },
    };
    Ok(layout_in(
        Nav::Decks,
        "Edit card",
        Some(user),
        html! {
            (flash(msg))
            @if let Some(url) = from.study_url() {
                (parent(&url, "Back to studying"))
            } @else if let Some(d) = decks.iter().find(|d| d.id == card.deck_id) {
                (parent(&format!("/decks/{}", d.id), &deck_plain(&d.name)))
            }
            h1 { "Edit card" }
            (form_error(error))
            (coach_box(&card, &flags))
            (card_form(&format!("/cards/{id}{}", from.query()), &decks, &draft, "Save", false, &hints))
            (card_schedule(&card, &items, decay, &clock))
            (card_actions(&card))
            (card_log(&history, &clock))
        },
    ))
}

fn card_schedule(card: &Card, items: &[crate::multi::CardItem], decay: f32, clock: &scheduler::Clock) -> Markup {
    let r = scheduler::retrievability(&card.sched(), decay, clock);
    html! {
        h2 { "Scheduling" }
        dl.kv {
            dt { "State" } dd { (state_label(card.card_state())) @if card.suspended { " (suspended)" } }
            @if card.card_state() != CardState::New {
                dt { "Due" } dd { (capitalize(&format::when(clock, card.due))) }
            }
            @if let Some(r) = r { dt { @if card.multi.is_some() { "Weakest item now" } @else { "Recall chance now" } } dd { (format::percent(Some(f64::from(r)))) } }
        }
        @if !items.is_empty() {
            h2 { "Items" }
            ol.item-recall {
                @for i in items {
                    li {
                        span.text { (multi::title(&i.text)) }
                        span.chance {
                            @match scheduler::retrievability(&i.sched(), decay, clock) {
                                Some(r) => (format::percent(Some(f64::from(r)))),
                                None => "New",
                            }
                        }
                    }
                }
            }
        }
        details.more {
            summary { "Scheduling details" }
            dl.kv {
                @if let Some(s) = card.stability { dt { "Stability" } dd { (format!("{s:.1} days")) } }
                @if let Some(d) = card.difficulty { dt { "Difficulty" } dd { (format!("{d:.1} / 10")) } }
                dt { "Reviews / lapses" } dd { (card.reps) " / " (card.lapses) }
                dt { "Added" } dd { (source_label(&card.created_by)) }
            }
        }
    }
}

fn card_actions(card: &Card) -> Markup {
    let id = card.id;
    html! {
        form.actions method="post" action=(format!("/cards/{id}/{}", if card.suspended { "unsuspend" } else { "suspend" })) {
            button.btn type="submit" { @if card.suspended { "Unsuspend" } @else { "Suspend" } }
        }
        details.danger {
            summary { "Forget progress or delete this card…" }
            form method="post" action=(format!("/cards/{id}/delete")) {
                p { "Forgetting makes it a new card again. Deleting removes it and its history." }
                div.actions {
                    button.btn.danger type="submit" formaction=(format!("/cards/{id}/forget")) { "Forget progress" }
                    button.btn.danger type="submit" { "Delete card" }
                }
            }
        }
    }
}

fn card_log(history: &[serde_json::Value], clock: &scheduler::Clock) -> Markup {
    html! {
        @if !history.is_empty() {
            h2 { "History" }
            table.history {
                thead { tr { th { "When" } th { "Answer" } th { "Next" } } }
                tbody {
                    @for h in history.iter().rev() {
                        tr {
                            td { (capitalize(&format::when(clock, h["timestamp"].as_i64().unwrap_or(0)))) }
                            td {
                                @if let (Some(asked), Some(recalled)) = (h["items_asked"].as_i64(), h["items_recalled"].as_i64()) {
                                    (recalled) " of " (asked) " remembered"
                                } @else {
                                    (rating_label(h["rating"].as_str().unwrap_or("")))
                                }
                                @if h["source"] == "agent" { " (by an agent)" }
                            }
                            td { (h["next_interval"].as_str().unwrap_or("")) }
                        }
                    }
                }
            }
        }
    }
}

pub(crate) async fn card_page(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Query(from): Query<CardFrom>,
    Flash(msg): Flash,
) -> WebResult {
    Ok(card_view(&app, &user.0, id, &from, msg.as_deref(), None, None).await?.into_response())
}

pub(crate) async fn card_update(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Query(from): Query<CardFrom>,
    mp: Multipart,
) -> WebResult {
    let access = user.access();
    let mut f = read_card_form(mp).await?;
    let result = async {
        let front_image = match f.front_upload.take() {
            Some(d) => Patch::Set(Some(app.store_image(&access, d).await?.id)),
            None if f.remove_front_image => Patch::Set(None),
            None => Patch::Keep,
        };
        let back_image = match f.back_upload.take() {
            Some(d) => Patch::Set(Some(app.store_image(&access, d).await?.id)),
            None if f.remove_back_image => Patch::Set(None),
            None => Patch::Keep,
        };
        app.update_card(
            &access,
            id,
            CardInput {
                front: Some(f.front.clone()),
                back: Some(f.back.clone()),
                front_image,
                back_image,
                multi: Patch::Set(f.multi),
            },
        )
        .await?;
        if let Some(deck) = f.deck
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
            let msg = with_image_hint(fixable(&e).ok_or(e)?, &f);
            Ok(form_again(card_view(&app, &user.0, id, &from, None, Some(&f), Some(&msg)).await?))
        }
    }
}

pub(crate) async fn card_action(
    State(app): State<AppState>,
    user: WebUser,
    Path((id, action)): Path<(i64, String)>,
) -> WebResult {
    let access = user.access();
    or_back(
        format!("/cards/{id}"),
        async {
            let msg = match action.as_str() {
                "suspend" => {
                    app.set_suspended(&access, id, true).await?;
                    "Card suspended."
                }
                "unsuspend" => {
                    app.set_suspended(&access, id, false).await?;
                    "Card unsuspended."
                }
                "forget" => {
                    app.reset_card(&access, id).await?;
                    "Card progress forgotten; it's new again."
                }
                "delete" => {
                    let card = app.card(&access, id).await?;
                    app.delete_card(&access, id).await?;
                    return Ok(redirect_msg(format!("/decks/{}", card.deck_id), "Card deleted."));
                }
                "learn-items" => {
                    app.update_card(
                        &access,
                        id,
                        CardInput { multi: Patch::Set(Some(Split::List)), ..Default::default() },
                    )
                    .await?;
                    "Now learned item by item: you mark what you remembered, and each item keeps its own schedule."
                }
                keep if keep.starts_with("keep-") => {
                    let kind =
                        coach::Kind::parse(&keep["keep-".len()..]).ok_or_else(|| AppError::bad("unknown action"))?;
                    app.dismiss_flag(&access, id, kind).await?;
                    "Kept as is."
                }
                _ => return Err(AppError::bad("unknown action")),
            };
            Ok(redirect_msg(format!("/cards/{id}"), msg))
        }
        .await,
    )
}

/// The card being written as it will look when studied.
pub(crate) fn card_preview(front: &str, back: &str, how: Option<Split>) -> Markup {
    let pieces = how.and_then(|h| multi::split(back, h));
    html! {
        section.face.front { (face(front, None)) }
        hr.divider;
        @match &pieces {
            Some(p) => section.face.back.list { (multi_back(p, &[], false)) },
            None => section.face.back { (face(back, None)) },
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct PreviewForm {
    /// The card being edited, so it doesn't count as like itself.
    pub(crate) card: Option<i64>,
    #[serde(default)]
    pub(crate) front: String,
    #[serde(default)]
    pub(crate) back: String,
    #[serde(default)]
    pub(crate) multi: String,
}

/// For the card form while you write: how the card will look, and which
/// ways the back can be learned item by item.
pub(crate) async fn card_preview_json(
    State(app): State<AppState>,
    user: WebUser,
    Form(f): Form<PreviewForm>,
) -> WebResult {
    let splits: Vec<&str> = multi::splits(&f.back).into_iter().map(Split::as_str).collect();
    let how = Split::parse(&f.multi);
    let html = card_preview(&f.front, &f.back, how).into_string();
    let hints = write_hints(&app, &user.access(), &f.front, &f.back, how, f.card).await?;
    Ok((
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({ "splits": splits, "html": html, "hints": hint_lines(&hints).into_string() }).to_string(),
    )
        .into_response())
}
