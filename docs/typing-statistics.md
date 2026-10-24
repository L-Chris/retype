# Typing statistics

Settings > Statistics shows input committed through retype. English quantities are actual words and English speeds are words per minute, not five-character WPM equivalents. Chinese quantities remain characters. Desktop and Android totals stay separate after cloud synchronization; characters and words are never added into a single total or stacked count bar.

## Word boundaries

Both adapters use the shared `retype-types::statistics::WordCounter`. Its English-specific word-boundary tailoring groups Latin letters, combining accents, and digits, keeps internal straight/curly apostrophes, and splits on hyphens and other punctuation. A token must contain a Latin letter: `Hello world` is two words, `don't` and `GPT4` are one each, `hello-world` is two, and numbers, emoji, and Chinese text do not count as English words. Dictionary membership and spelling correctness are irrelevant.

Only successful commits reach the counter. It carries an unfinished token across fragmented commits, finishes at word/editor boundaries, and can undo unfinalized direct characters on backspace. Candidate selection, including the automatic trailing space, counts once; cancelled composition and rejected writes contribute nothing. Finished words measure cumulative input activity, not the document's current contents: deleting a previously finished word does not subtract it. Editing existing words cannot reconstruct their complete document context from the input stream alone.

Clipboard paste, cross-device paste, AI translation replacements, and text from another input method are excluded. Windows hidden/protected contexts and Android password/private editor policies remain excluded. A Windows host that rejects synchronous input can handle the original key itself, which retype cannot count accurately.

## Speed and history

Speed divides input quantity by active input time separately for each language, excluding successive input gaps longer than 15 seconds. English needs at least five words and ten seconds of effective input; Chinese needs ten characters and ten seconds. Windows live speed uses the last five minutes and hides after 30 seconds without a commit. Period speeds use sums of counts and time, not averages of individual speeds, grouped by local day, Monday-based week, month, or year.

Legacy English character records are retained without estimating word counts. New word quantities and their own effective time start after upgrading, so earlier character-only periods have no comparable English word speed. The desktop reader accepts both old five-column and new seven-column numeric logs; Android migrates its statistics database from version 1 to 2 without clearing records.

## Storage and synchronization

Windows queues numeric deltas outside TSF transactions to `%LOCALAPPDATA%\retype\statistics`; Android writes numeric minute buckets asynchronously. Neither persists input text or the unfinished word. The shared counter holds only bounded character classes in memory; Android carries this numeric state over JNI.

Cloud buckets keep the legacy `english`/`english_ms` fields and add `english_words`/`english_word_ms`, defaulting to zero when absent. Absolute per-device/per-stream/per-minute totals merge with component-wise maxima, and local-restoration differences include the new fields. Repeated imports do not duplicate activity, and legacy snapshots do not clear known word totals. Upgrade both devices to see word metrics on both; older clients continue to use their character-based display.

## Versions

All statistics pages default to **All versions** on opening. The version selector
also offers each version with recorded activity and **Historical / unmarked**
for older records. The selector applies to quantities, voice totals, recent and
period speeds, charts, and (on Windows) input measurements. It combines with the
existing calendar period; versions do not change the calendar boundaries.

Each producer stamps records with its compiled application version, including
prerelease suffixes. An old input component still running after an upgrade keeps
its own version. Historical CSV rows and imported buckets without version evidence
remain unmarked; neither installation time nor the current version is used to
relabel them. Build metadata is not an additional UI filter.

New desktop log lines are JSON records containing `second`, `version`, and numeric
`counts`; readers also accept the existing five/seven-column CSV lines. New log
stream names contain `@<version>` before `.log`, which preserves version attribution
through older cloud clients that drop unknown JSON fields. Android database version
3 adds version to the minute bucket primary key and preserves the numeric Windows
measurements when forwarding snapshots. Both v1 and v2 databases migrate without
clearing history.

Cloud identity is `(device, stream, minute, version)`. Repeated imports merge each
absolute counter by maximum, including input-length counters; local restoration
subtracts already available counters with the same identity. All-version rates
are calculated from summed numerators and denominators, not averages of version
rates. Upgrade clients to display the new record format and filters.

## Windows spelling revisions and input lengths

These measurements start with the new collector; old quantities and time cannot
be used to estimate them. They are collected on Windows first. Other platforms
retain their existing quantities and speeds with version filtering.

A composition round starts when the input method begins composing and ends on a
successful host commit or cancellation. Partial candidate choices remain within
the round. An actual Backspace reduction of the spelling buffer marks that round
as revised. Reopening a chosen segment, candidate navigation, and automatic
candidate refresh do not mark it. Multiple deletions in one round contribute one
revised round, with deletion actions kept as a separate numeric counter.

The spelling revision rate is `revised successful rounds / successful rounds`.
It describes editing behavior, not whether the user's text was incorrect. Esc,
deleting the whole composition, and losing an unfinished composition contribute
cancellations, outside the success denominator. Failed/rejected host writes are
excluded. The Chinese-mode raw-letter Enter escape retains its existing exclusion
and is not counted as a cancellation. Deleting text that is already committed is
outside this metric.

Length is measured from the successfully committed unit: Han characters for
Chinese and the shared English word counter for English, excluding punctuation
and whitespace. A mixed-language unit contributes to each applicable language's
own distribution; language totals are never combined as a single unit count.
Direct English fragments accumulate to a word boundary rather than counting each
character write as an input. Backspace within that unfinished direct word marks
its observed revision; passthrough host editing cannot verify the document's final
state. Previously completed words are not reconstructed after cursor movement.

Buckets are `1`, `2`, `3`, `4`, `5–8`, `9–16`, `17–32`, and `33+`. Each language and
bucket holds successful input count, revised input count, and total output units.
The UI shows count share, revision rate, and mean length for the selected version
and current day/week/month/year. No successful samples means an unavailable rate
(`—`), not zero percent. These are commit-length distributions, not linguistic
word frequencies or reconstructed complete sentences.

Only bounded in-memory numeric/class state is added to the input path. Final
numeric deltas are queued without waiting for disk, a document scan, UI Automation,
or another process. No persistent field ID, raw sentence, or keystroke history is
required. The existing hidden/protected context policy and exclusions for voice,
paste, and AI replacement remain in force.
