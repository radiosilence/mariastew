### Added

- **Security headers, which this served none of.** No CSP, no `nosniff`, no
  `frame-ancestors`, no referrer policy — every response went out bare. The
  policy is the one mcp-gateway already uses, so the two agree rather than each
  having their own idea of what is allowed.

  `unsafe-eval` is in it and is not optional: Datastar compiles each `data-*`
  expression with `Function()`, so without it no attribute on any page does
  anything. It buys an attacker nothing on its own — `script-src 'self'` still
  refuses script from anywhere else — but it does mean escaping is the only
  thing between a string that reaches the DOM and code that runs, which is what
  `escaping_is_only_opted_out_of_where_expected` now watches: Askama escapes by
  default, and the single opt-out is a sub-template's rendered markup rather
  than a value.

  Nothing needed relaxing to fit: no inline handlers, no inline styles, no
  `data:` URIs, and every script and image already came from this origin.
