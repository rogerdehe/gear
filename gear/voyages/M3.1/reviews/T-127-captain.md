# T-127 attempt 2: captain's judgment of the review

Round 2 passed every rework item: the whole suite runs 935/935 in 37 s, no test over 10 s, the three named merge tests at 7.6-8.4 s, engine lib 31 s -> 16 s, the switch settle moved behind the Herdr boundary, the strict timestamp assertion restored. The only blocking finding, F5, is a pre-existing 10 ms poll in onboard::tests::an_empty_model_list_is_waited_for (model_list_poll_ms = 10): it costs nothing measurable, and the one rework round is used. The captain passes it; F5 (and F4) go to the M4 backlog.
