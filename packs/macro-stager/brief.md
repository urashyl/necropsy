A purchasing clerk opened a Word "purchase order" and enabled macros, and endpoint tooling flagged Word making a web request. The macro source is `PO-88231_macro.bas.txt` (text only, it cannot run).

The macro calls `po-portal.invalid`, which does not resolve. Send the request it would send to your instance, with every header it sets. The server answers only a client that looks like the macro. The reply holds a key. Submit it as the flag.
