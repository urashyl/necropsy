The answer is `k.morgan`.

Any domain user can ask for a service ticket for any account with an SPN, and part of that ticket is encrypted with the service account's password. Take the ticket away, guess passwords offline, and the domain controller never sees a wrong attempt. That's Kerberoasting. Asking for RC4 (`0x17`) instead of AES (`0x12`) makes the guessing far faster, so roasting tools ask for it by default.

At 14:22:07, `k.morgan` asks for tickets to thirteen service accounts in about five seconds, all RC4, from 10.0.9.14. Every other request from `k.morgan` that day comes from 10.0.4.30 for file shares and the intranet, in AES. So either k.morgan suddenly needed SQL, SharePoint, backup, Jira and the AD sync account at once, or someone on 10.0.9.14 has their password.

The two other things that stand out aren't it. At 02:00, `svc-scanner` requests tickets for every service account too, but that's the vulnerability scanner's nightly run, and it uses AES. And `svc-legacyapp` gets an RC4 ticket for `svc-sql` every hour on the hour from the same app server. That's an old application that only supports RC4, and somebody should fix it, but it isn't an attack.

```bash
grep ',0x17,' 4769.csv | cut -d, -f3,6 | sort | uniq -c | sort -rn
```

Next: find out what 10.0.9.14 is and how k.morgan's password got there, and assume the attacker will crack any service account with a weak password. Change those passwords to long random ones (or use managed service accounts), and turn off RC4 for accounts that don't need it.
