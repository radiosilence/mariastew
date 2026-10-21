### Removed

- The "Added" toast, which never went away. `data-on-signal-patch` is one of Datastar's value-returning attributes, so it wraps the last `;`-separated chunk of the expression in `return (…)` — `{ $addStatus = ''; $addMessage = '' }` compiles to `return ($addMessage = '' })`, a syntax error, so the handler that cleared the signal never existed. It is not replaced: the metadata row is on screen from the moment an add is accepted (0.1.24), so the banner promising one "shortly" was covering the row that already proved it ([#350](https://github.com/radiosilence/jaritanet/issues/350))

### Fixed

- Closing the add dialog clears `$addStatus` whatever it held. A success was deliberately left set for the toast to keep reading, which made the toast the only thing that ever cleared it — and the dialog closes itself on a success, so an `ok` outliving one attempt shuts the next dialog as soon as any signal moves ([#350](https://github.com/radiosilence/jaritanet/issues/350))
