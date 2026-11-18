//! Chess cards. The front is a position, the back the move that answers it.
//!
//! Everything lives in the card's two sides, so search, sharing, merging,
//! export and agents work without special cases:
//!
//! - front: the position as FEN on the first line, then optionally a blank
//!   line and a question or hint (Markdown);
//! - back: the right moves on the first line in SAN, separated by commas
//!   (`Nf3, Ng5`), then optionally a blank line and a comment (Markdown).
//!
//! Moves are accepted in SAN or UCI and always saved and shown as SAN.
//! Legality, SAN and UCI come from `cozy-chess` (MIT).

use cozy_chess::{Board, Color, GameStatus, Move, Piece, Square, util};

use crate::{
    App,
    auth::Access,
    error::{AppError, Result},
    kind::CardKind,
    scheduler::Rating,
    service::Card,
};

/// A chess card read from its two sides.
#[derive(Debug, Clone)]
pub struct Chess {
    pub board: Board,
    /// The question under the position; may be empty.
    pub prompt: String,
    pub moves: Vec<Move>,
    /// The comment under the moves; may be empty.
    pub comment: String,
}

/// The first line, and the rest after it with surrounding blank lines gone.
fn head_and_rest(text: &str) -> (&str, &str) {
    let text = text.trim();
    match text.split_once('\n') {
        Some((head, rest)) => (head.trim(), rest.trim()),
        None => (text, ""),
    }
}

/// Reads a position. The pieces and the side to move are enough (EPD has
/// four fields, a full FEN six); an en passant square
/// that no pawn can take is dropped, as many tools write one after every
/// double step.
pub fn parse_fen(fen: &str) -> Result<Board, String> {
    let fields: Vec<&str> = fen.split_whitespace().collect();
    if fields.len() < 2 {
        return Err("the first line of the front must be a position in FEN, like \
                    rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1"
            .into());
    }
    // Without castling and en passant fields, there are none.
    let castling = fields.get(2).copied().unwrap_or("-");
    let ep = fields.get(3).copied().unwrap_or("-");
    let clocks = match fields.get(4..6) {
        Some([a, b]) if a.parse::<u16>().is_ok() && b.parse::<u16>().is_ok() => format!("{a} {b}"),
        _ => "0 1".into(),
    };
    let full = |ep: &str| format!("{} {} {castling} {ep} {clocks}", fields[0], fields[1]);
    match Board::from_fen(&full(ep), false) {
        Ok(b) => Ok(b),
        Err(cozy_chess::FenParseError::InvalidEnPassant) => {
            Board::from_fen(&full("-"), false).map_err(|e| fen_problem(&e))
        }
        Err(e) => Err(fen_problem(&e)),
    }
}

fn fen_problem(e: &cozy_chess::FenParseError) -> String {
    use cozy_chess::FenParseError as E;
    match e {
        E::InvalidBoard => "the FEN's position isn't possible (check the pieces, the kings and who is in check)",
        E::InvalidSideToMove => "the FEN's second field must be w or b",
        E::InvalidCastlingRights => "the FEN's castling rights don't match where the kings and rooks stand",
        _ => "the FEN can't be read",
    }
    .into()
}

/// One move in SAN (`Nf3`, `exd5`, `O-O`, `0-0`, `e8=Q`, `Nf3+!`) or UCI
/// (`g1f3`, `e1g1`, `e7e8q`). `None` when it isn't legal here.
pub fn parse_move(board: &Board, text: &str) -> Option<Move> {
    // Move numbers ("12." or "12...") and annotations ("!?") are not part of it.
    let t = text.trim();
    let digits = t.len() - t.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let t = if digits > 0 && t[digits..].starts_with('.') { t[digits..].trim_start_matches('.').trim_start() } else { t };
    let t = t.trim_end_matches(['!', '?']);
    if t.is_empty() {
        return None;
    }
    let b = t.as_bytes();
    let uci = (4..=5).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1].is_ascii_digit()
        && b[2].is_ascii_lowercase()
        && b[3].is_ascii_digit();
    if uci {
        if let Ok(mv) = util::parse_uci_move(board, &t.to_ascii_lowercase()) {
            if board.is_legal(mv) {
                return Some(mv);
            }
        }
    }
    let mut san = t.replace('0', "O");
    // A promotion written in lower case: e8=q.
    if let Some(i) = san.find('=') {
        san = format!("{}{}", &san[..i], san[i..].to_ascii_uppercase());
    }
    util::parse_san_move(board, &san).ok()
}

pub fn san(board: &Board, mv: Move) -> String {
    util::display_san_move(board, mv).to_string()
}

/// Standard UCI: castling is the king's two-square step (`e1g1`).
pub fn uci(board: &Board, mv: Move) -> String {
    util::display_uci_move(board, mv).to_string()
}

/// The moves on a line: separated by commas, spaces, slashes or "or".
fn parse_moves(board: &Board, line: &str) -> Result<Vec<Move>, String> {
    let mut moves = Vec::new();
    for word in line.split([',', ' ', '/', ';']).filter(|w| !w.is_empty() && *w != "or") {
        let mv = parse_move(board, word).ok_or_else(|| format!("“{word}” isn't a legal move in this position"))?;
        if !moves.contains(&mv) {
            moves.push(mv);
        }
    }
    if moves.is_empty() {
        return Err("the first line of the back must be the move that answers it, like Nf3".into());
    }
    Ok(moves)
}

/// Reads a chess card's sides, saying what is wrong when they don't fit.
pub fn read(front: &str, back: &str) -> Result<Chess, String> {
    let (fen, prompt) = head_and_rest(front);
    let board = parse_fen(fen)?;
    if board.status() != GameStatus::Ongoing {
        return Err("the game is already over in this position, so there is no move to find".into());
    }
    let (line, comment) = head_and_rest(back);
    let moves = parse_moves(&board, line)?;
    Ok(Chess { board, prompt: prompt.into(), moves, comment: comment.into() })
}

/// The two sides as they are saved: the full FEN, and the moves in SAN.
pub fn tidy(front: &str, back: &str) -> Result<(String, String), String> {
    let c = read(front, back)?;
    let with = |head: String, rest: &str| if rest.is_empty() { head } else { format!("{head}\n\n{rest}") };
    Ok((with(c.board.to_string(), &c.prompt), with(c.answer_san(), &c.comment)))
}

/// "White to move" or "Black to move".
pub fn to_move(board: &Board) -> &'static str {
    match board.side_to_move() {
        Color::White => "White to move",
        Color::Black => "Black to move",
    }
}

/// How a chess card is named in lists: its question, else whose move it is.
pub fn title(front: &str) -> String {
    let (fen, prompt) = head_and_rest(front);
    match (prompt.lines().next(), parse_fen(fen)) {
        (Some(line), _) => line.trim().trim_start_matches('#').trim().to_string(),
        (None, Ok(b)) => format!("Chess: {}", to_move(&b).to_lowercase()),
        (None, Err(_)) => "Chess position".into(),
    }
}

fn mates(board: &Board, mv: Move) -> bool {
    let mut after = board.clone();
    after.play_unchecked(mv);
    after.status() == GameStatus::Won
}

/// The result of a move played on a chess card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub right: bool,
    pub played: Move,
}

impl Chess {
    /// The right moves in SAN, as saved: "Nf3, Ng5".
    pub fn answer_san(&self) -> String {
        self.moves.iter().map(|&m| san(&self.board, m)).collect::<Vec<_>>().join(", ")
    }

    /// Grades a played move; `None` when it isn't legal here. When a right
    /// move mates, every mating move is right.
    pub fn grade(&self, played: &str) -> Option<Verdict> {
        let mv = parse_move(&self.board, played)?;
        let right = self.moves.contains(&mv)
            || (mates(&self.board, mv) && self.moves.iter().any(|&m| mates(&self.board, m)));
        Some(Verdict { right, played: mv })
    }

    pub fn uci_moves(&self) -> Vec<String> {
        self.moves.iter().map(|&m| uci(&self.board, m)).collect()
    }

    /// The pieces for a page without the board script: rank 8 first, a1 last.
    pub fn squares(&self) -> Vec<(Square, Option<char>)> {
        let mut out = Vec::with_capacity(64);
        for rank in (0..8).rev() {
            for file in 0..8 {
                let sq = Square::index(rank * 8 + file);
                let glyph = self.board.piece_on(sq).zip(self.board.color_on(sq)).map(|(p, c)| glyph(p, c));
                out.push((sq, glyph));
            }
        }
        out
    }
}

fn glyph(piece: Piece, color: Color) -> char {
    let white = ['♙', '♘', '♗', '♖', '♕', '♔'];
    let black = ['♟', '♞', '♝', '♜', '♛', '♚'];
    let i = Piece::ALL.iter().position(|&p| p == piece).unwrap_or(0);
    if color == Color::White { white[i] } else { black[i] }
}

impl Verdict {
    /// Chess cards grade themselves: a right move is Good, a wrong one Again.
    pub fn rating(&self) -> Rating {
        if self.right { Rating::Good } else { Rating::Again }
    }
}

impl App {
    /// A chess card and what the move played on it is worth. Answering is
    /// left to the caller, with [`Verdict::rating`].
    pub async fn play_move(&self, access: &Access, card_id: i64, played: &str) -> Result<(Card, Chess, Verdict)> {
        let card = self.card(access, card_id).await?;
        let chess = of_card(&card)?;
        let verdict = chess
            .grade(played)
            .ok_or_else(|| AppError::bad(format!("“{}” isn't a legal move in this position", played.trim())))?;
        Ok((card, chess, verdict))
    }
}

/// A chess card's position and answer.
pub fn of_card(card: &Card) -> Result<Chess> {
    if card.kind != CardKind::Chess {
        return Err(AppError::bad("this isn't a chess card"));
    }
    read(&card.front, &card.back).map_err(AppError::bad)
}

// ----- importing -------------------------------------------------------------

/// Positions from a pasted list or file, as (front, back) pairs already
/// tidied, and the lines that could not be read with their problem.
///
/// Each line is EPD (`<FEN> bm Nf3; c0 "comment";`), or a FEN followed by
/// the moves and an optional comment after `;`, `|` or a tab. Lichess's
/// puzzle CSV (`PuzzleId,FEN,Moves,…`) works too: its first move is the
/// opponent's, and the second is the answer.
pub fn parse_positions(text: &str) -> (Vec<(String, String)>, Vec<(usize, String)>) {
    let text = text.trim_start_matches('\u{feff}');
    let lichess = text.lines().next().is_some_and(|l| l.starts_with("PuzzleId,FEN,Moves"));
    let mut cards = Vec::new();
    let mut problems = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") || (lichess && i == 0) {
            continue;
        }
        let parsed = if lichess { lichess_line(line) } else { position_line(line) };
        match parsed.and_then(|(front, back)| tidy(&front, &back)) {
            Ok(card) => cards.push(card),
            Err(e) => problems.push((i + 1, e)),
        }
    }
    (cards, problems)
}

/// EPD or "FEN moves; comment", as raw (front, back).
fn position_line(line: &str) -> Result<(String, String), String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.len() < 5 {
        return Err("needs a FEN and a move".into());
    }
    let mut n = 4;
    if words.len() > 6 && words[4].parse::<u16>().is_ok() && words[5].parse::<u16>().is_ok() {
        n = 6;
    }
    let fen = words[..n].join(" ");
    // The rest of the line after the FEN's last field.
    let mut rest = line;
    for w in &words[..n] {
        rest = rest[rest.find(w).map_or(0, |at| at + w.len())..].trim_start();
    }
    if let Some(epd) = epd_ops(rest) {
        return Ok((fen, epd));
    }
    let (moves, comment) = match rest.find([';', '|', '\t']) {
        Some(at) => (&rest[..at], rest[at + 1..].trim()),
        None => (rest, ""),
    };
    Ok((fen, format!("{}\n\n{comment}", moves.trim())))
}

/// EPD operations: `bm` (best moves) and `c0`…`c9` (comments). `None`
/// when the text isn't EPD.
fn epd_ops(rest: &str) -> Option<String> {
    let mut moves = None;
    let mut comments = Vec::new();
    let mut s = rest.trim();
    while !s.is_empty() {
        let (op, after) = s.split_once(char::is_whitespace).unwrap_or((s, ""));
        if !op.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return None;
        }
        let after = after.trim_start();
        // An operand list ends at a semicolon outside quotes.
        let mut quoted = false;
        let end = after.char_indices().find(|&(_, c)| {
            if c == '"' {
                quoted = !quoted;
            }
            c == ';' && !quoted
        });
        let (operands, next) = match end {
            Some((at, _)) => (&after[..at], &after[at + 1..]),
            None => (after, ""),
        };
        match op {
            "bm" => moves = Some(operands.trim().to_string()),
            c if c.len() == 2 && c.starts_with('c') => comments.push(operands.trim().trim_matches('"').to_string()),
            _ => {}
        }
        s = next.trim();
    }
    moves.map(|m| format!("{m}\n\n{}", comments.join("\n\n")))
}

/// One Lichess puzzle: the opponent's move is played first, the answer is
/// the next one. Its themes become the comment.
fn lichess_line(line: &str) -> Result<(String, String), String> {
    let cols: Vec<&str> = line.split(',').collect();
    let (Some(fen), Some(moves)) = (cols.get(1), cols.get(2)) else {
        return Err("needs a FEN and moves".into());
    };
    let mut board = parse_fen(fen)?;
    let mut ucis = moves.split_whitespace();
    let (Some(first), Some(answer)) = (ucis.next(), ucis.next()) else {
        return Err("needs the opponent's move and the answer".into());
    };
    let mv = parse_move(&board, first).ok_or("the first move isn't legal")?;
    board.play_unchecked(mv);
    let themes = cols.get(7).map(|t| t.split_whitespace().collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let comment = if themes.is_empty() { String::new() } else { format!("Themes: {themes}") };
    Ok((board.to_string(), format!("{answer}\n\n{comment}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITALIAN: &str = "r1bqkbnr/pppp1ppp/2n5/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4";
    /// White mates with Qxf7# (Scholar's mate).
    const SCHOLAR: &str = "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 4 4";

    fn board(fen: &str) -> Board {
        parse_fen(fen).unwrap()
    }

    #[test]
    fn moves_parse_as_san_or_uci() {
        let b = board(ITALIAN);
        let castle = parse_move(&b, "O-O").unwrap();
        assert_eq!(parse_move(&b, "0-0"), Some(castle));
        assert_eq!(parse_move(&b, "e1g1"), Some(castle));
        assert_eq!(uci(&b, castle), "e1g1");
        assert_eq!(san(&b, castle), "O-O");
        assert_eq!(parse_move(&b, "Ng5"), parse_move(&b, "f3g5"));
        assert_eq!(parse_move(&b, "4. Ng5!?").map(|m| san(&b, m)), Some("Ng5".into()));
        assert_eq!(parse_move(&b, "Nf3"), None, "no knight can go there");
        assert_eq!(parse_move(&b, "e2e5"), None);
        assert_eq!(parse_move(&b, "hello"), None);
    }

    #[test]
    fn promotions_in_any_case() {
        let b = board("8/4P3/8/8/8/k7/8/K7 w - - 0 1");
        let q = parse_move(&b, "e8=Q").unwrap();
        assert_eq!(parse_move(&b, "e8=q"), Some(q));
        assert_eq!(parse_move(&b, "e7e8q"), Some(q));
        assert_eq!(san(&b, parse_move(&b, "e8N").unwrap()), "e8=N");
    }

    #[test]
    fn fen_is_lenient_about_clocks_and_en_passant() {
        assert!(parse_fen("rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3").is_ok());
        assert!(parse_fen("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq -").is_ok());
        assert!(parse_fen("6k1/8/8/8/8/8/8/6K1 b").is_ok());
        assert!(parse_fen("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR").unwrap_err().contains("FEN"));
        assert!(parse_fen("What is this?").unwrap_err().contains("FEN"));
        assert!(parse_fen("8/8/8/8/8/8/8/8 w - -").is_err(), "no kings");
    }

    #[test]
    fn tidy_saves_full_fen_and_san() {
        let (front, back) = tidy(
            "  r1bqkbnr/pppp1ppp/2n5/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq -\n\n\nFind the plan  ",
            "e1g1 or f3g5\n\nBoth are fine.",
        )
        .unwrap();
        assert_eq!(front, format!("{}\n\nFind the plan", ITALIAN.replace("4 4", "0 1")));
        assert_eq!(back, "O-O, Ng5\n\nBoth are fine.");
        assert_eq!(tidy(ITALIAN, "").unwrap_err(), "the first line of the back must be the move that answers it, like Nf3");
        assert!(tidy(ITALIAN, "Nf3").unwrap_err().contains("“Nf3”"));
        // Saving again changes nothing.
        assert_eq!(tidy(&front, &back).unwrap(), (front, back));
    }

    #[test]
    fn grading_takes_both_notations_and_any_mate() {
        let c = read(SCHOLAR, "Qxf7#").unwrap();
        assert!(c.grade("h5f7").unwrap().right);
        assert!(c.grade("Qxf7").unwrap().right);
        assert!(!c.grade("Nc3").unwrap().right);
        assert_eq!(c.grade("Ke3"), None);
        // Two mates: both count when the answer mates.
        let c = read("6k1/5ppp/8/8/8/8/8/R2R2K1 w - - 0 1", "Ra8").unwrap();
        assert!(c.grade("Rd8").unwrap().right);
        assert!(!c.grade("Rd7").unwrap().right);
        // Several right moves.
        let c = read(ITALIAN, "O-O, Ng5").unwrap();
        assert!(c.grade("e1g1").unwrap().right && c.grade("Ng5").unwrap().right);
        assert_eq!(c.uci_moves(), ["e1g1", "f3g5"]);
    }

    #[test]
    fn finished_games_are_refused() {
        let mated = "r1bqkb1r/pppp1Qpp/2n2n2/4p3/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 0 4";
        assert!(read(mated, "Ke7").unwrap_err().contains("over"));
    }

    #[test]
    fn titles() {
        assert_eq!(title(&format!("{ITALIAN}\n\n## A quiet plan\nmore")), "A quiet plan");
        assert_eq!(title(ITALIAN), "Chess: white to move");
    }

    #[test]
    fn import_reads_epd_plain_lines_and_lichess() {
        let text = format!(
            "# my puzzles\n\
             {SCHOLAR} bm Qxf7#; id \"scholar\"; c0 \"The classic trap.\";\n\
             r1bqkbnr/pppp1ppp/2n5/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4 Ng5, O-O | Attack f7\n\
             {ITALIAN} Ke3\n"
        );
        let (cards, problems) = parse_positions(&text);
        assert_eq!(cards.len(), 2);
        assert!(cards[0].1.starts_with("Qxf7#\n\nThe classic trap."), "{}", cards[0].1);
        assert_eq!(cards[1].1, "Ng5, O-O\n\nAttack f7");
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].0, 4);

        let lichess = "PuzzleId,FEN,Moves,Rating,RatingDeviation,Popularity,NbPlays,Themes,GameUrl,OpeningTags\n\
            00008,r6k/pp2r2p/4Rp1Q/3p4/8/1N1P2R1/PqP2bPP/7K b - - 0 24,f2g3 e6e7 b2b1 b3c1,1760,80,83,72,crushing hangingPiece long middlegame,https://lichess.org/787zsVup/black#47,\n";
        let (cards, problems) = parse_positions(lichess);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(cards[0].1, "Rxe7\n\nThemes: crushing, hangingPiece, long, middlegame");
        assert!(cards[0].0.contains(" w "), "white answers after black's move");
    }
}
