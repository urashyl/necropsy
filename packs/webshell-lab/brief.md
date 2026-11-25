A small gardening site was compromised and the host was imaged before it was cleaned up. Two things survived: a slice of its access log, and the attacker's webshell, recovered from the uploads folder (saved here as `thumbs.php.sample`, so it cannot run by accident).

We rebuilt the site as it was, **with the shell still in place**. It is a real web server running in an isolated container, started just for you. Start it below, then use the attacker's own shell against it, the way they did. They staged what they stole in a hidden place on the box.

Read the sample to learn how to talk to it, find the stash, and submit the `exfil_key` it holds as the flag. Nothing on that box can reach the internet, and it is deleted when you are done.
