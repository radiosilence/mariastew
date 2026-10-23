### Fixed

- A stream that dies while the page is open comes back on its own. #386 covered the case with a trigger — the page went away and returned — and an ordinary deploy has none: the pod is replaced rather than drained, every open stream ends, and Datastar reads a finished 200 body as the request being over, so a laptop tab left in front of you showed a frozen list until it was reloaded ([#390](https://github.com/radiosilence/jaritanet/issues/390))

### Added

- The stream sends a `datastar-heartbeat` frame every five seconds when it has nothing else to say, and the page reconnects when three of them fail to arrive (`ms.streamLost`). Silence had to be given a meaning first: the stream sends the rows whose hash moved, so a queue with nothing running is silent because there is nothing to report, and a client cannot tell that apart from a socket that stopped carrying bytes without erroring. Datastar's own view of whether the request is live is the one thing this cannot consult — in that case it is wrong, and there is nothing else to ask ([#390](https://github.com/radiosilence/jaritanet/issues/390))

- A 401 stops the watchdog rather than starting a loop. A session lives only in the pod's memory, so the deploy that ends the stream is usually the deploy that signs the page out, and `require_session` answers a Datastar request with a status where a navigation would get the login redirect. No reconnect gets past that, and the asking is itself one of the frames that resets the clock — so the page would have issued a doomed request every sixteen seconds for as long as the tab stayed open. It resumes if a later reconnect is actually served ([#390](https://github.com/radiosilence/jaritanet/issues/390))

### Changed

- The heartbeat replaces axum's keep-alive. Both stop a proxy timing out an idle connection; only one of them reaches the page, and two mechanisms firing at different rates is one more thing to hold in mind than there needs to be ([#390](https://github.com/radiosilence/jaritanet/issues/390))
