title: Fitting FSRS to your memory
group: scheduling
evidence: fsrs
summary: What optimizing does, when it helps, and why AiNKI only keeps better fits.

FSRS starts with default parameters. They come from millions of reviews by many people, and they work well for most. Your memory and your cards are your own, though. Optimizing fits the parameters to your review history.

## When it helps

With a few hundred reviews the fit starts to beat the defaults. It keeps improving as your history grows. AiNKI optimizes by itself about once a month, once you have at least 400 reviews and 100 new ones since the last run. You can also press Optimize now in settings.

## Only better fits are kept

A new fit is not always better. With little data it can be worse. So AiNKI measures how well the old and the new parameters predict your past reviews, and keeps the new ones only if they predict better. Settings say what the last run did.

## Separate parameters per preset

Material differs. Vocabulary can behave differently from anatomy. Once a preset has about 400 reviews of its own, AiNKI fits parameters for it too, starting from yours. Decks with that preset then use them. Presets with less history use your general parameters.

## Is it working?

Stats have a section called How well FSRS knows you. It compares the recall FSRS expected with the recall you had, grouped by how sure FSRS was. Close numbers mean a good fit. If you forget a lot more than expected, optimizing usually helps.

## Sources

- The FSRS wiki: "The Algorithm" and "Optimizer" (github.com/open-spaced-repetition/fsrs4anki/wiki)
- The SRS benchmark: github.com/open-spaced-repetition/srs-benchmark

=== cards
What does optimizing FSRS do? :: It fits the scheduler's parameters to your own review history.
How often does AiNKI optimize by itself? :: About once a month, once there are enough new reviews.
When does AiNKI keep newly fitted parameters? :: Only when they predict your past reviews better than the old ones.
When does a preset get parameters of its own? :: Once it has about 400 reviews.
Where can you check how well FSRS predicts your memory? :: In Stats, under "How well FSRS knows you".
