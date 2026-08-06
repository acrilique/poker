// Externalized from shell.html so the shell's Content-Security-Policy can ship
// without 'unsafe-inline' in script-src (which would otherwise neuter the CSP
// against reflected/stored XSS). Served as a normal same-origin script.
if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("/poker/sw.js", { scope: "/poker/" });
}
