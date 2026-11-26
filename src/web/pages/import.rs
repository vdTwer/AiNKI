//! Importing Anki packages and CSV files.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

pub(crate) async fn import_page(
    State(app): State<AppState>,
    user: WebUser,
    Flash(msg): Flash,
) -> WebResult {
    let decks = app.decks(&user.access()).await?;
    Ok(layout_in(
        Nav::Add,
        "Import",
        Some(&user.0),
        html! {
            (add_head("import", "Import", flash(msg.as_deref())))
            h2 { "From Anki (.apkg)" }
            p { "In Anki, use " strong { "File → Export…" } ", choose " em { "Anki Deck Package (*.apkg)" } " and tick " em { "Include scheduling information" } " to keep your progress. Upload the file here and its decks are created for you." }
            p.muted.small { "Each card keeps the first two fields of its note, the first image of each side and its review history. Cloze notes become one card per gap. " code { ".colpkg" } " backups work too." }
            form method="post" action="/import" enctype="multipart/form-data" {
                input type="hidden" name="kind" value="apkg";
                label.file.drop {
                    span.drop-text { "Choose an Anki file" }
                    span.muted.small { ".apkg or .colpkg, or drop it here" }
                    input type="file" name="file" accept=".apkg,.colpkg" required;
                }
                div.form-foot.end { button.btn.primary type="submit" { "Import Anki deck" } }
            }
            h2 { "From a CSV file" }
            p { "One card per line: front, back. Commas, semicolons or tabs work. A header line \"front,back\" is skipped." }
            @if decks.is_empty() {
                p { "CSV files go into an existing deck. " a href="/decks/new" { "Create a deck first." } }
            } @else {
                form method="post" action="/import" enctype="multipart/form-data" {
                    input type="hidden" name="kind" value="csv";
                    div.deck-line {
                        label.deck-pick { span.deck-label { "Into deck" }
                            span.select { select name="deck" { @for d in &decks { option value=(d.id) { (deck_plain(&d.name)) } } } }
                        }
                    }
                    div.sheet.paste {
                        label.face-label for="csv-text" { "Paste the cards" }
                        textarea.csv id="csv-text" name="text" rows="5" placeholder="Hund,dog\nKatze,cat" {}
                        label.file.attach {
                            span.attach-text { "Or choose a file" }
                            " " span.muted.optional { "(.csv, .tsv, .txt)" }
                            input type="file" name="file" accept=".csv,.tsv,.txt";
                        }
                    }
                    div.form-foot.end { button.btn type="submit" { "Import CSV" } }
                }
            }
        },
    )
    .into_response())
}

pub(crate) async fn import_upload(
    State(app): State<AppState>,
    user: WebUser,
    mp: Multipart,
) -> WebResult {
    let access = user.access();
    let mut fields = match read_multipart(mp).await {
        Ok(f) => f,
        Err(e) => return or_back("/import", Err(e)),
    };
    let file = fields.remove("file");
    let text = |k: &str| field_text(&fields, k);
    // The file has to be picked again anyway, so problems come back as a
    // message on the import page.
    let report = async {
        Ok::<_, AppError>(match text("kind").as_str() {
            "apkg" => {
                let data = file
                    .filter(|d| !d.is_empty())
                    .ok_or_else(|| AppError::bad("choose a file"))?;
                app.import_apkg(&access, data).await?
            }
            "csv" => {
                let deck: i64 = text("deck")
                    .parse()
                    .map_err(|_| AppError::bad("pick a deck"))?;
                let mut body = String::from_utf8_lossy(
                    file.as_deref().unwrap_or_default(),
                )
                .to_string();
                if body.trim().is_empty() {
                    body = text("text");
                }
                app.import_csv(&access, deck, &body).await?
            }
            _ => return Err(AppError::bad("unknown import type")),
        })
    }
    .await;
    let report = match report {
        Ok(r) => r,
        Err(e) => return or_back("/import", Err(e)),
    };
    let mut msg = format!("Imported {}", count(report.cards_imported, "card"));
    if report.reviews_imported > 0 {
        msg += &format!(", {}", count(report.reviews_imported, "review"));
    }
    if report.images_imported > 0 {
        msg += &format!(", {}", count(report.images_imported, "image"));
    }
    if !report.decks_created.is_empty() {
        let names: Vec<String> =
            report.decks_created.iter().map(|n| deck_plain(n)).collect();
        msg += &format!(". New decks: {}", names.join(", "));
    }
    if report.skipped > 0 {
        msg += &format!(". Skipped {} empty", report.skipped);
    }
    msg.push('.');
    Ok(redirect_msg("/", &msg))
}
