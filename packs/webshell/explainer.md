The answer is `/wp-content/plugins/fancy-uploader/tmp/wp-cache-71c2.php`.

A blog like this gets scanned all day. Three addresses work through the usual list (`/shell.php`, `/wso.php`, `/.env`, `/wp-content/uploads/up.php` and so on), get 404s and move on. Those aren't the incident. They're people hoping someone else already did the hard part.

At 13:02, 203.0.113.50 does something different. It asks for `readme.txt` of the `fancy-uploader` plugin, which is how you check which version a site runs, and 40 seconds later posts to the plugin's upload handler. Then it requests `/wp-content/uploads/2026/11/wp-cache-71c2.php` and gets a 403, because this server doesn't run PHP from the uploads folder. So the attacker posts again with `dest=tmp`, the plugin saves the same file into its own `tmp` folder, and that copy runs. The test request `?c=aWQ=` is base64 for `id`, and the 54-byte response is exactly the length of `uid=33(www-data) gid=33(www-data) groups=33(www-data)` plus a newline.

Two and a half hours later a different address, 198.51.100.140, starts posting to that file. The access log doesn't record POST bodies, so the commands are hidden, but the response sizes vary like command output does. One of them is about the size of `wp-config.php`. That's the database password.

When you clean this up, deleting the file isn't enough. Update or remove the plugin, change the database password and the WordPress salts, and look for whatever the later POSTs downloaded. Two of them returned nothing at all, which is what you'd see from a command that starts something in the background.
