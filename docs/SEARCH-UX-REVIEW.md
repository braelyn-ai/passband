# Search UX investigation — September 17, 2026

Initial design proposal, based on the supplied Passband and Superhuman screenshots and
the code in this checkout. The companion `search-ux-preview.html` is a static,
responsive layout study, not a working search implementation. Example status
assignments and shortened previews are illustrative, not fetched mailbox data.

## Implementation — September 18

The macOS search panel now uses the proposed continuous rows, responsive sender
columns, readable previews, warm match highlights and separate status groups.
Keyword and recall results carry done status and FTS-matched surface forms;
new clients tolerate older daemons without inventing a status. URLs are shortened
to readable domains and paths, without tracking parameters or opaque IDs.
Subscription footers are trimmed. Sender avatars appear on the left.

By default, typing requests keyword retrieval with unfinished-first ordering,
including sent mail to preserve the old hybrid search's mailbox scope.
`Include related` enables hybrid retrieval and remembers the choice across
searches and app launches. Existing results remain on screen while a new request
runs. Returning from the reader preserves every loaded page, its cursor and selection. Keyword
ordering is exact across status and strict/partial boundaries, with one store
lock per page and no first-page seam count. Related retrieval starts with a
page-sized recall window and expands as needed (capped at 600). Its cursor
remembers delivered IDs so expanding the ranking does not repeat rows, and it
exhausts the bounded window before entering the done group.
It is still approximate recall and does not enumerate all mailbox matches.

The API logs queue, retrieval, diagnostics and total time for searches taking
at least 250 ms, without queries or mail content. The original 30-second delay
has not been reproduced; the change removes embedding from typing's critical
path rather than claiming that the root cause was measured.

Both the client and daemon must be updated for unfinished-first grouping and
server-derived highlights. An older daemon still supports the new layout and
keyword requests, but status remains unknown. Nothing has been deployed.

Search now opens wide by default and returns to wide results when the reader
closes. The sidebar layout remains available alongside an open message. Escape
closes search directly. A visible `Ask agent` action and ⌘Return run any nonempty
query without waiting for classification or keyword results; only hits fetched
for the current query are supplied as context. The Off preference and the
in-flight guard still apply to explicit requests.

## Why the comparison reads differently

Superhuman's screenshot gives the eye stable sender and date columns, then a
continuous subject/preview reading line. Passband makes every email a separate
rounded card with three different text levels. The reader repeatedly moves down
and across, and the most useful evidence is the least readable text.

In `passband/Sources/Passband/Views/SidePanel.swift`, `HitRow` uses `inkFaintest`
for the snippet and timestamp, `inkDim` for the subject, and strongest ink for
the sender. Repeated senders dominate this particular query. The dark snippet
color is #62707F; the glass background makes its effective contrast variable.
The screenshot also shows long tracking URLs and joined sentences consuming
the limited preview space.

Highlighting currently splits the raw query on spaces and paints every
case-insensitive substring in sender, subject and snippet. This highlights
`Anjuna` in repeated names and tracking domains, but does not follow the
server's stemming, quoting or operator parsing. It uses the same accent wash
as row selection. A highlight therefore does not reliably explain retrieval.

The server already attempts 24-token FTS body windows, including for hybrid
hits. This is a presentation and window-quality problem, not simply a missing
snippet feature. Substring highlighting alone cannot explain semantic matches.

## Proposed presentation

- Use a continuous list with fine separators and a predictable row height.
  In a wide view, align sender, subject/preview, status and date. In the narrow
  panel, keep sender/date on one line, subject on the next and at most two
  readable preview lines. Remove the large rounded background on every row.
- Give subjects primary ink and previews readable secondary ink. Reserve the
  faintest ink for nonessential decoration. Done mail stays readable.
- Show a short passage around useful matched words, preferring prose over
  tracking URLs and subscription footers. Preserve the actual message text;
  mark omitted spans with ellipses. Never hide a URL when the query targets it.
  Fix HTML-to-text word boundaries at extraction where possible; legacy data
  may need separate treatment.
- Use a restrained warm highlight with high-contrast text for match spans;
  keep blue for interaction and unfinished status. Sender matches remain
  possible but should not overwhelm subject/body evidence. Return match spans
  from retrieval rather than trying to reconstruct FTS semantics in Swift.
- If a result has no lexical evidence, label it quietly as a related result
  rather than manufacturing an apparent exact match.
- `Recent` currently means recency-weighted relevance, not chronological
  ordering. Rename it to `Relevance + recency` (or explain it beside the sort
  control) so the dates do not make the list look incorrectly sorted.

## Not done first

The user's requested default is a hard grouping: **Not done**, then **Done**.
Within each group retain the chosen relevance/recency ordering. This includes
partial matches: an unfinished partial match precedes a done strict match.
Do not silently reinterpret this as a small score boost.

Use a small blue leading bar and a group heading for unfinished mail. Done
rows get a small checkmark and their own heading. Selection gets a distinct
row background and outline. Do not use bold to mean unfinished: it can be
confused with unread. Provide accessible status labels, not just color.

`SearchHit` currently carries no done status in either Rust or Swift, so the
client cannot implement this faithfully on its own. Add status from the
account-scoped `triage` join. An untriaged message is not done; an absent status
from an older daemon is unknown and should not be represented as confirmed
unfinished. Use the existing message-level status semantics; do not invent a
thread aggregation rule as part of this change.

Apply grouping **before pagination**, on the server. Keyword mode currently
concatenates strict and partial blocks, so sorting individual SQL blocks by
status is insufficient: the order must be unfinished strict, unfinished
partial, done strict, done partial. Filter-only listing needs the same status
ordering. Hybrid and semantic retrieval use bounded candidate windows; a
stable status partition of each fetched page is insufficient and can strand
unfinished hits on later pages. Use a stable result snapshot/cursor or
status-aware candidate retrieval so pagination does not move the group seam.
Retain account, spam and sealed exclusions throughout. Keep the existing
agent search behavior separate if this preference is only for human search.

## Latency: evidence and limits

Confirmed from code:

1. The panel waits 220 ms after a keystroke, then requests 50 results with
   `partial=1` and no explicit retrieval mode.
2. With an attached embedder, the API defaults to hybrid.
3. `hybrid_search_legs_ordered` embeds the query, runs vector retrieval, then
   keyword retrieval, fusion, hydration and snippets. Diagnostics also finish
   before the response. No keyword result can reach the UI early.
4. The embedding session is protected by a mutex. Lazy loading and concurrent
   ingest embedding can delay a query. The lazy embedder's documented reload
   is about 200 ms, which alone does **not** explain 30 seconds.
5. The view cancels obsolete Swift tasks. The server uses `spawn_blocking`;
   cancellation of an HTTP request does not establish that already-running
   embedding/SQL work stops. Repeated requests can still consume server work.
6. On closer inspection, the loading indicator is in the field; the result
   scroller stays mounted while searching. The session retains the current
   query, but there is no multi-query result cache.

A read-only SQL probe of the available local 1,694-message database used the
screenshot query (`"anjuna" AND "tickets"*`, then OR), the account/spam/sealed
guards, weighted BM25 and 24-token snippets, limited to 50 rows:

| Query | First run | Next three runs |
| --- | --- | --- |
| All terms | 12.06 ms | 0.78, 0.68, 0.67 ms |
| Any term | 44.10 ms | 4.28, 4.08, 4.09 ms |

This is an isolated SQL measurement, not end-to-end search. The local database
has the older `fts5(subject, body)` schema, not this checkout's newer tokenizer.
The measurement excludes embedding, full hybrid work, diagnostics, contention,
transport and rendering. No matching request timing was found in the local
daemon log. The reported 30-second delay remains unreproduced and unattributed.

## Implementation order and acceptance

1. Instrument total request time plus queue wait, embedding, vector retrieval,
   keyword retrieval, hydration/snippets and diagnostics. Measure the actual
   active daemon, including cold search, warm search and search during ingest.
   Record timings and result counts without logging mail bodies or query text.
2. Give as-you-type search a keyword-first response, independent of the
   embedder. Preserve semantic discovery as a subsequent, bounded phase. Do
   not let late responses overwrite a newer query, change the selected item,
   or insert unfinished results beneath already displayed done results. Keep
   semantic additions separate until deliberately incorporated into a stable
   result ordering. Ensure both phases have consistent sent-mail eligibility.
3. Implement status-aware ordering and wire status through to the UI. Test a
   page boundary containing both strict/partial matches and both statuses,
   both sort choices, filter-only queries and hybrid candidates. Changing done
   status while search is open must invalidate the result snapshot.
4. Implement the layout and evidence treatment shown in the companion study.
   Validate narrow/wide layouts, light/dark appearance, long sender names,
   quoted phrases, stems, operator queries and semantic-only results.

Initial performance target: first usable warm results within 300 ms of the
last keystroke, with keyword server work below 100 ms at p95 on the real
mailbox. These are acceptance targets, not measured claims. Only reduce the
debounce after measuring; changing 220 ms cannot fix a 30-second stall.
