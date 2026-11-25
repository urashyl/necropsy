The answer is `r.alvarez@example.invalid`.

Between 01:12 and about 04:00, every account gets exactly one attempt, a few minutes apart, from a rotating set of addresses in 203.0.113.0/24 located in five countries. Almost all of them fail with `50126`, wrong password. The user agent is `BAV2ROPC` on every one, which is how the logs show legacy authentication that sends the password directly, like SMTP AUTH or an old desktop client, rather than a browser sign-in.

Two attempts don't fail that way. `t.nguyen` gets `50074` and then `500121`: the password was right, MFA was required, and MFA didn't happen. Nobody got in, but the attacker now knows that password, so reset it anyway. `r.alvarez` gets `0`, success, over `Authenticated SMTP`, with `singleFactorAuthentication` and conditional access `notApplied`. SMTP AUTH can't do MFA, and this tenant still allows it, so one correct password was enough. The attacker comes back three more times over the same route afterwards.

`h.olsen` is the distraction. Ten failures in a row from their usual London address at 9am and then a lockout is someone who forgot their password.

```bash
jq -r 'select(.userAgent=="BAV2ROPC") | [.createdDateTime,.userPrincipalName,.clientAppUsed,.status.errorCode] | @tsv' signins.jsonl
```

The fix is mostly policy. Block legacy authentication, which turns this whole technique off. Then reset r.alvarez's and t.nguyen's passwords, end r.alvarez's sessions, and check their mailbox for forwarding rules, which is usually the first thing an attacker sets up.
