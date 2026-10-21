### Fixed

- A restored download resumes from the torrent on disk rather than asking the swarm for metadata it already has, so a deploy no longer leaves rows on "finding peers" over data that is already complete ([#359](https://github.com/radiosilence/jaritanet/issues/359))
- An add interrupted by a restart is picked up and finished on startup. It came back paused — that is how aria2 serialises one — with nothing left to start it or to narrow it to the files worth keeping ([#359](https://github.com/radiosilence/jaritanet/issues/359))
- An add no longer loses the race with its own pause. aria2 refuses to unpause a group until it has finished stopping, and a single attempt left the download stopped for good ([#359](https://github.com/radiosilence/jaritanet/issues/359))
- A restored torrent being re-hashed says "checking files" rather than "downloaded". aria2 counts every piece as complete before it starts checking them, so the state that answered first was the wrong one for the longest wait a deploy has ([#359](https://github.com/radiosilence/jaritanet/issues/359))
- The bar and the bytes beside it follow the hash check while one runs, rather than sitting at whatever the download reached ([#359](https://github.com/radiosilence/jaritanet/issues/359))
