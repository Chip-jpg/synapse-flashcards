-- Indexes on `review_logs`, which had none besides its primary key. Forward-
-- only. Adds two indexes; no table, row, or rule changes.
--
-- Every rating appends a row, so the table grows to hundreds of thousands of
-- rows over a few years of reviewing, and these questions read all of it:
-- - which cards were rated today (`reviewed_at`): buried cloze siblings,
--   asked whenever a session shows or rates a card and for every deck's due
--   count, plus today's count on the Study Desk and the statistics' 30 days;
-- - one card's ratings (`card_id`): its Again streak, asked for every card a
--   session shows.
-- With the indexes each reads only the rows it needs. The answers are the
-- same; only how SQLite finds them changes.

CREATE INDEX review_logs_by_card ON review_logs (card_id);

CREATE INDEX review_logs_by_time ON review_logs (reviewed_at);
