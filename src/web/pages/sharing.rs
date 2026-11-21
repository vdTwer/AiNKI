//! Offering a deck to someone, and accepting or declining an offer.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

#[derive(Deserialize)]
pub(crate) struct ShareForm {
    pub(crate) user: String,
}

pub(crate) async fn deck_share(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Form(f): Form<ShareForm>,
) -> WebResult {
    or_back(
        format!("/decks/{id}/edit#share"),
        async {
            let share = app.share_deck(&user.access(), id, &f.user).await?;
            Ok(redirect_msg(
                format!("/decks/{id}/edit#share"),
                &format!("Shared with {}. They'll see it on their home page.", share.to),
            ))
        }
        .await,
    )
}

pub(crate) async fn share_action(
    State(app): State<AppState>,
    user: WebUser,
    Path((id, action)): Path<(i64, String)>,
) -> WebResult {
    let access = user.access();
    or_back(
        "/",
        async {
            match action.as_str() {
                "accept" => {
                    let deck = app.accept_share(&access, id).await?;
                    Ok(redirect_msg(format!("/decks/{}", deck.id), "Shared deck added to your decks."))
                }
                "decline" => {
                    app.decline_share(&access, id).await?;
                    Ok(redirect_msg("/", "Share declined."))
                }
                "cancel" => {
                    let deck = app.cancel_share(&access, id).await?;
                    Ok(redirect_msg(format!("/decks/{deck}/edit#share"), "Share cancelled."))
                }
                _ => Err(AppError::bad("unknown action")),
            }
        }
        .await,
    )
}
