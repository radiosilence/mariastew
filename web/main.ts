// `ms` has to exist before Datastar first scans the document and compiles the
// expressions that call it. Datastar is askama_datastar's copy, loaded by its
// own module script after this one in page.html; module scripts run in the
// order they appear.
import "./ms";
