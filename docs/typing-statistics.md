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
