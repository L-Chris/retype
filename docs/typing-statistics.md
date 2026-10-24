# Typing statistics

Settings > Statistics shows input committed through retype. English quantities are actual words and English speeds are words per minute, not five-character WPM equivalents. Chinese quantities remain characters. Desktop and Android totals stay separate after cloud synchronization; characters and words are never added into a single total or stacked count bar.

## Word boundaries

All three adapters use the shared `retype-types::statistics::WordCounter`. Its English-specific word-boundary tailoring groups Latin letters, combining accents, and digits, keeps internal straight/curly apostrophes, and splits on hyphens and other punctuation. A token must contain a Latin letter: `Hello world` is two words, `don't` and `GPT4` are one each, `hello-world` is two, and numbers, emoji, and Chinese text do not count as English words. Dictionary membership and spelling correctness are irrelevant.

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
period speeds, charts, and input measurements. It combines with the
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

## Spelling revisions and input lengths

Windows, macOS, and Android use the shared `InputTracker`. It retains numeric
round state and bounded character classes, never raw sentences or key history.
Partial candidate choices remain in one round. An actual spelling-buffer reduction
on Backspace marks the round as revised; reopening a chosen part, paging, and
candidate refresh do not. Multiple deletions count as one revised round, with
separate numeric deletion-action counts.

Only confirmed successful commits enter the success denominator. Windows counts
after its host edit succeeds; macOS holds numeric measurements with its commit
token until acknowledgement; Android updates the collector through opaque JNI
handles and persists after the editor transaction succeeds. Stale/replayed
acknowledgements do not duplicate activity. Failed controlled writes are excluded.
Cancellation, deleting an entire composition, and losing an unfinished round are
counted separately. Cancellation has no final length and is not assigned to a
language histogram. Direct English fragments accumulate to a word boundary;
passthrough editing is an observed sequence and cannot establish the host's final
document state. Edits to already completed text are not reconstructed.

Collection stores **20 buckets per language**: exact lengths 1 through 19 and a
final **20+** bucket for lengths >=20. Each contains successful input count,
revised input count, and actual output-unit sum. The tail retains its true unit
sum, so means never use a bucket midpoint or treat long inputs as exactly 20.
Chinese uses Han characters; English uses shared English word boundaries.
Punctuation and whitespace do not add length. A mixed-language unit contributes
to each applicable language's own distribution.

The wire field is `counts.input_detail`. The earlier grouped `counts.input`
measurements are not read or migrated. Android database version 4 clears only the
old length-measurement column; quantities, time, versions, and other history remain.
New detailed counters participate in absolute max merges and local restoration
just like the existing statistics. Input callbacks do not wait for disk or inspect
the host document to collect these measurements.

## Distribution charts

Each platform shows one Chinese and one English combination chart. Display
buckets `1`, `2`, `3`, `4`, `5–10`, and `11+` are aggregated at query time from exact
collection buckets. Future display boundaries below 20 can be changed without
recapturing input; the 20+ tail cannot be split retrospectively.

Bars show input share on the left axis. An orange line shows revision rate on the
right axis. Both axes are fixed at 0–100%; input counts appear in tooltips. No successful samples means no rate point, and lines break
across empty groups. A tooltip combines count, share, revised count, rate, and true
mean length; samples below five are marked as small. Desktop supports hover and
click-to-pin, and Android supports touch and drag between groups. Escape dismisses
pinned desktop details. Summary/help tooltips contain totals, weighted rate, mean,
and cancellations rather than additional tables or permanent text blocks.

Version (default all) and calendar-period filters apply to both charts. Rates are
computed after summing their raw numerator/denominator; no averaging percentages.
Changing filters clears old point selections. These are commit-length
distributions, not linguistic word frequency or reconstructed sentence statistics.
