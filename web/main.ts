// Evaluated in import order: `ms` has to exist before Datastar first scans the
// document and compiles the expressions that call it.
import "./ms";
import "./vendor/datastar.js";
