# Scheduling and studying

AiNKI uses the [FSRS](https://github.com/open-spaced-repetition/fsrs4anki/wiki/The-Algorithm) model through the [`fsrs`](https://crates.io/crates/fsrs) crate (FSRS-6). FSRS tracks each card's *stability* (how long until recall drops to 90%) and *difficulty*. It schedules the next review for when your chance of recalling the card drops to your *desired retention*.

By default there are two answer buttons, **Again** (forgot) and **Good** (remembered). FSRS works well with two buttons. You can turn on **Hard** and **Easy** in settings.

## Presets

New accounts get these presets:

| Preset | Retention | New/day | Reviews/day | Learning steps | Use for |
| --- | --- | --- | --- | --- | --- |
| Standard | 90% | 20 | 200 | 10m | Most material. |
| Exam prep | 95% | 50 | 9999 | 1m 10m | An upcoming test. Set an exam date on the deck so no card is scheduled after it. |
| Language vocabulary | 90% | 30 | 300 | 10m | Many small cards every day. |
| Relaxed | 85% | 10 | 100 | 10m | Long-term learning on a small time budget. |
| Maintenance | 80% | 0 | 100 | 10m | Keeping old knowledge with minimal effort. |

How these were chosen, following the FSRS project's guidance:

- **Default parameters.** The FSRS-6 defaults are fitted on about 10,000 Anki collections. Once you have a few hundred reviews, AiNKI fits FSRS to your own history about once a month by itself (or press **Optimize now** in settings). New parameters are kept only when they predict your past reviews better than the old ones. A preset with about 400 reviews of its own gets its own parameters.
- **Desired retention.** 0.80 to 0.95 is the useful range. Below about 0.80 you relearn so much that total work goes up. Above 0.95, work climbs steeply: 0.95 costs about twice the reviews of 0.90. Where the least work lies differs from person to person, so each preset's page simulates the next 30, 90 or 365 days at several retentions with your own memory and speed.
- **Learning steps.** Keep them short (minutes, under a day). Spacing across days is FSRS's job; long learning steps work against the model.
- **Daily limits.** Each new card leads to about 10 reviews in its first month, so the review limit should be at least 10 times the new card limit.
- **Fuzz.** Intervals get a small random spread so cards added together don't stay due together.

## Studying

- **Timed cards.** A preset can set a time limit for cards you have seen before. When time runs out the card counts as forgotten; **+10 s** adds time once. Answer times are never used to grade otherwise.
- **Typed answers.** A preset can ask you to type the answer first. What you wrote is shown above the answer; you still decide whether you remembered.
- **Sure check.** Optionally say whether you are sure before the answer is shown. It doesn't change the schedule; stats show how often you were right when you felt sure.
- **Quiet hours.** No cards during the hours you choose, on the website (a rest screen, with "Study anyway") or through agents.
- **Breaks.** Optionally suggest a break after a set number of minutes.
- **Catch-up mode.** After time away: no new cards, a daily cap on reviews, and the cards you most likely still remember first. AiNKI offers it when many cards are overdue.
- **Mixed study.** Study all decks at once, or one at a time. Mixing is offered for convenience; there is no good evidence it helps with unrelated material.

## Import from Anki

Upload an `.apkg` or `.colpkg` under **Import**. Each card keeps the first two fields of its note (HTML stripped, Markdown-ish formatting) and the first image on each side. Reversed cards swap the fields, and cloze notes become one card per gap. Review history is imported, so FSRS memory states are computed from your real history and the optimizer can use it. Suspended cards stay suspended.

CSV import takes one card per line (`front,back`). Commas, semicolons or tabs all work.
