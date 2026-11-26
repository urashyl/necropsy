This is a "Magecart" style skimmer, malicious JavaScript injected into a checkout page. It is the most common card theft on the web, and it is almost always obfuscated just enough to defeat a `grep`.

The loader here uses the three cheapest tricks: strings kept in an array instead of inline, one of them base64 *and reversed* (`atob` of the reverse), one as hex. Decoding the array gives the gate's path and a key the gate expects, so only the skimmer can talk to it. The skimmer hooks the form's `submit` event, copies every named field into an object, and sends it as a base64 JSON blob in the URL of an `Image` request. An image request needs no CORS permission and no response, which is why attackers prefer it.

The giveaway is a leftover. A debug function, `_dbg`, calls the same gate with `dump=1`, which makes it return what it holds. Attackers reuse the same panel code to read their loot back. When you find a collector, ask what the author's own tooling would call.

Defenders: Subresource Integrity on third-party scripts, a strict `script-src`, and watching for a new script origin on the payment page are what stop this.
