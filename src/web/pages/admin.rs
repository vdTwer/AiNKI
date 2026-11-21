//! The admin panel: who invited whom, and the invitation tree.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

/// Who invited whom, drawn as a tree that grows to the right: each account
/// a dot with its name, the ones it invited on a branch below it. Accounts
/// nobody invited (the first ones, the admin) are the roots. A filled dot
/// studied in the last 30 days; a ring didn't.
/// One account in the invitation graph: its row, and how deep it hangs.
struct Node<'a> {
    a: &'a invites::Account,
    depth: usize,
    row: usize,
    parent_row: Option<usize>,
}

/// The accounts in graph order, depth first: each takes the next row,
/// under the one who invited it. An inviter that is gone makes its people
/// roots.
fn graph_rows(accounts: &[invites::Account]) -> Vec<Node<'_>> {
    let ids: std::collections::HashSet<i64> = accounts.iter().map(|a| a.id).collect();
    let mut children: HashMap<Option<i64>, Vec<&invites::Account>> = HashMap::new();
    for a in accounts {
        let parent = a.invited_by.filter(|p| ids.contains(p) && *p != a.id);
        children.entry(parent).or_default().push(a);
    }
    let mut nodes: Vec<Node> = Vec::new();
    let mut stack: Vec<(&invites::Account, usize, Option<usize>)> =
        children.get(&None).map(|r| r.iter().rev().map(|a| (*a, 0, None)).collect()).unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    while let Some((a, depth, parent_row)) = stack.pop() {
        if !seen.insert(a.id) {
            continue;
        }
        let row = nodes.len();
        nodes.push(Node { a, depth, row, parent_row });
        if let Some(kids) = children.get(&Some(a.id)) {
            stack.extend(kids.iter().rev().map(|k| (*k, depth + 1, Some(row))));
        }
    }
    nodes
}

pub(crate) fn invite_graph(accounts: &[invites::Account]) -> Markup {
    const ROW: f64 = 34.0;
    const INDENT: f64 = 30.0;
    const TOP: f64 = 18.0;
    const LEFT: f64 = 8.0;
    let nodes = graph_rows(accounts);
    let x = |d: usize| num::usize_f64(d).mul_add(INDENT, LEFT);
    let y = |r: usize| num::usize_f64(r).mul_add(ROW, TOP);
    let width = nodes
        .iter()
        .map(|n| num::usize_f64(n.a.handle().len()).mul_add(8.6, x(n.depth) + 16.0) + 110.0)
        .fold(320.0_f64, f64::max)
        .ceil();
    let height = (y(nodes.len().max(1)) - ROW + TOP).ceil();
    let active = now() - 30 * 86_400;
    html! {
        div.graph-wrap {
            svg.invite-graph width=(width) height=(height) viewBox=(format!("0 0 {width} {height}")) role="img"
                aria-label=(format!("Invitation graph of {} accounts", nodes.len())) {
                @for n in &nodes {
                    @if let Some(pr) = n.parent_row {
                        // Down from the inviter's dot, then a soft turn right to this one.
                        @let (px, py, cy, cx) = (x(n.depth - 1), y(pr) + 6.0, y(n.row), x(n.depth) - 6.0);
                        path.edge d=(format!("M{px} {py}V{:.1}Q{px} {cy} {:.1} {cy}H{cx}", cy - 9.0, px + 9.0)) {}
                    }
                }
                @for n in &nodes {
                    @let (cx, cy) = (x(n.depth), y(n.row));
                    @let studied = n.a.last_review.is_some_and(|t| t >= active);
                    g.node.admin[n.a.is_admin] {
                        circle.dot.idle[!studied && !n.a.is_admin] cx=(cx) cy=(cy) r="4.5" {}
                        text x=(cx + 14.0) y=(cy + 5.0) {
                            tspan.name { (n.a.username) }
                            @if n.a.is_admin { tspan.tag { " (admin)" } } @else { tspan.tag { "#" (format!("{:04}", n.a.discriminator)) } }
                        }
                    }
                }
            }
        }
    }
}

/// "Today", "Yesterday", "24 Sep", "24 Sep 2025": for tables.
pub(crate) fn short_date(clock: &scheduler::Clock, ts: i64) -> String {
    let d = clock.date_of(ts);
    match (clock.today() - d).num_days() {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        _ if chrono::Datelike::year(&d) == chrono::Datelike::year(&clock.today()) => d.format("%-d %b").to_string(),
        _ => d.format("%-d %b %Y").to_string(),
    }
}

pub(crate) async fn admin_page(
    State(app): State<AppState>,
    user: WebUser,
    headers: HeaderMap,
    Flash(msg): Flash,
) -> WebResult {
    let u = &user.0;
    if !u.is_admin {
        return Err(AppError::NotFound.into());
    }
    let accounts = app.accounts(u).await?;
    let inv = app.invites(u).await?;
    let clock = u.clock();
    let base = oauth::base_url(&app, &headers);
    let names: HashMap<i64, String> =
        accounts.iter().map(|a| (a.id, if a.is_admin { "admin".to_string() } else { a.handle() })).collect();
    let people: Vec<&invites::Account> = accounts.iter().filter(|a| !a.is_admin).collect();
    let month = now() - 30 * 86_400;
    let new_this_month = num::usize_i64(people.iter().filter(|a| a.created_at >= month).count());
    let studying = num::usize_i64(people.iter().filter(|a| a.last_review.is_some_and(|t| t >= month)).count());
    Ok(page(
        None,
        "Admin",
        Some(u),
        Some("admin"),
        html! {
            (flash(msg.as_deref()))
            h1 { "Accounts" }
            p.facts {
                (count(num::usize_i64(people.len()), "account")) " · "
                (format::number(new_this_month)) " joined in the last 30 days · "
                (format::number(studying)) " studied in that time"
            }
            h2 { "Who invited whom" }
            @if accounts.is_empty() { p.muted { "No accounts yet." } } @else { (invite_graph(&accounts)) }
            p.muted.small { "A filled dot studied in the last 30 days; a ring didn't." }
            h2 { "All accounts" }
            div.table-wrap {
                table.accounts {
                    thead { tr {
                        th { "Account" } th { "Joined" } th { "Invited by" }
                        th.num { "Cards" } th.num title="Reviews in the last 30 days" { "Reviews" } th.num { "Last studied" }
                    } }
                    tbody {
                        @for a in &people {
                            tr {
                                td { (a.handle()) }
                                td { (short_date(&clock, a.created_at)) }
                                td { (a.invited_by.and_then(|i| names.get(&i)).map_or(format::NONE, String::as_str)) }
                                td.num { (format::number(a.cards)) }
                                td.num { (format::number(a.reviews_30d)) }
                                td.num { (a.last_review.map_or_else(|| "Never".into(), |t| short_date(&clock, t))) }
                            }
                        }
                    }
                }
            }
            h2 { "Invite" }
            p { "The admin account can make as many links as it likes. Each works once, for 30 days." }
            (invite_links(&inv, &base, &clock))
            form.logout method="post" action="/logout" { button.link type="submit" { "Log out" } }
        },
    )
    .into_response())
}
