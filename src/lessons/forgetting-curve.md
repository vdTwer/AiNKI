title: Forgetting, and how FSRS models it
group: scheduling
evidence: strong
summary: Why memories fade, and the three numbers AiNKI keeps for every card.

In 1885 Hermann Ebbinghaus learned lists of nonsense syllables and tested himself after different delays. He forgot fast at first, then more slowly. That shape, the forgetting curve, has been found again many times since.

Every review changes the curve. After a successful recall the memory fades more slowly than before. This is why spaced repetition works: each review buys a longer gap until the next one.

## Three numbers per card

AiNKI uses FSRS, a scheduler fitted on millions of real reviews. It keeps three numbers for every card:

- Retrievability is the chance you remember the card right now. It starts high after a review and falls as time passes.
- Stability is how long the memory lasts. It is the number of days until retrievability falls to 90%. A card with a stability of 30 days has a 90% chance of being recalled a month after its last review.
- Difficulty says how hard the card is for you. Hard cards gain stability more slowly.

When a card comes up, FSRS looks at how much time has passed, updates these numbers from your answer, and picks the next date. The date is when retrievability will have fallen to the retention you aim for. See [desired retention](/learn/desired-retention).

## What follows from this

- Reviewing a card before it is due helps little. The memory was still strong.
- A card you forget loses most of its stability and starts again with short gaps.
- Cards you know well come back rarely. After a few years many of them come up less than once a year.

## Sources

- Ebbinghaus, H. (1885). Über das Gedächtnis.
- Murre, J. M. J., & Dros, J. (2015). Replication and analysis of Ebbinghaus' forgetting curve. PLOS ONE.
- Ye, J., Su, J., & Cao, Y. (2022). A stochastic shortest path algorithm for optimizing spaced repetition scheduling. KDD.
- The FSRS project: github.com/open-spaced-repetition

=== cards
What is a card's retrievability in FSRS? :: The chance you remember it right now. It falls as time passes since the last review.
What is a card's stability in FSRS? :: The number of days until the chance of recalling it falls to 90%.
What does difficulty do in FSRS? :: Hard cards gain stability more slowly after each review.
Why does reviewing a card early help little? :: The memory was still strong, so the review adds little stability.
Who first measured the forgetting curve? :: Hermann Ebbinghaus, in 1885.
