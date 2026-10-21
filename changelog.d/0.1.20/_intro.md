### Changed

- "Done" is "Clear" and "Delete download" is "Cancel & delete" — both now say what they do to the list, and a completed download reads "downloaded" rather than "ready" ([#354](https://github.com/radiosilence/jaritanet/issues/354))
- Every control that changes something on the server spins and disables itself for its own request, rather than waiting for the next snapshot to show the press landed ([#354](https://github.com/radiosilence/jaritanet/issues/354))
- Clearing holds the response open until aria2 has actually let go, and stops the download with `forceRemove` rather than waiting for the swarm to be told. A removal that does not take answers with a status instead of a silent 204 ([#354](https://github.com/radiosilence/jaritanet/issues/354))
- Use magnet icon for add button
- Every mutation is now bounded: 10s on any aria2 call, 10s on `mkdir`, and one second of retries on a removal ([#354](https://github.com/radiosilence/jaritanet/issues/354))
