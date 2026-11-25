The answer is `secure-examplebank-login.invalid`.

Hops 1 and 2 are ordinary redirects: a `302` with a `Location` header, then a page with `<meta http-equiv="refresh">`. Hop 3 is where it splits. It checks the user agent for iPhone, iPad or Android, checks the device has a touch screen, and picks one of two base64 strings:

```bash
echo 'aHR0cHM6Ly93d3cuZXhhbXBsZWJhbmsuaW52YWxpZC8=' | base64 -d    # everything else
echo '<the string in b>' | base64 -d                                # phones
```

Desktops go to the bank's real site. Phones go to `https://secure-examplebank-login.invalid/m/signin`.

The touch check is aimed at sandboxes like ours. Changing the user agent is easy and plenty of scanners do it, but a desktop browser pretending to be an iPhone still reports `maxTouchPoints` as 0, so it gets the real bank anyway.

This is called cloaking, and it's why "I clicked it and nothing happened" isn't enough to close a phishing report. The link was opened on the wrong kind of device. Real kits also filter on IP range, country, time of day and whether you've been before, and none of that shows up in one capture. Where you can, read the code on each hop instead of only following where it took you.
