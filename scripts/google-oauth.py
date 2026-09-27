#!/usr/bin/env python3
"""One-off: turn a Google OAuth client into a refresh token for serana.

Run it once, on the machine you can open a browser from. It prints the three values to put
in `.env` and never sends them anywhere but Google.

    python3 scripts/google-oauth.py

Nothing is installed: this uses only the standard library, so it works on a plain macOS or
Linux Python without a virtualenv.

Why `access_type=offline` and `prompt=consent` are both here: without the first Google
issues no refresh token at all, and without the second it issues one only on the very first
authorisation ever — so a second run after a mistake would silently hand back nothing.
"""

# Annotations as strings, so this runs on the Python that ships with macOS as well as on a
# current one: `str | None` in a class body is evaluated at runtime before 3.10.
from __future__ import annotations

import http.server
import json
import secrets
import socket
import sys
import threading
import urllib.parse
import urllib.request
import webbrowser

AUTH = "https://accounts.google.com/o/oauth2/v2/auth"
TOKEN = "https://oauth2.googleapis.com/token"
SCOPE = "https://www.googleapis.com/auth/calendar.events"


def free_port() -> int:
    """A port the loopback redirect can listen on."""
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Catcher(http.server.BaseHTTPRequestHandler):
    """Catches the one redirect Google makes back to us."""

    code: str | None = None
    state: str | None = None
    error: str | None = None

    def do_GET(self):  # noqa: N802 - the name is http.server's
        query = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
        Catcher.code = query.get("code", [None])[0]
        Catcher.state = query.get("state", [None])[0]
        Catcher.error = query.get("error", [None])[0]

        body = b"Authorised. You can close this tab and go back to the terminal."
        if Catcher.error:
            body = f"Refused: {Catcher.error}. Go back to the terminal.".encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass  # the server's own logging would bury the instructions


def from_json(path: str) -> "tuple[str, str]":
    """Read the two values out of the JSON the console downloads.

    Far safer than pasting: a client secret truncated by one character fails only at the
    very last step, after the browser dance has already succeeded, which is a miserable way
    to find out.
    """
    with open(path, encoding="utf-8") as f:
        blob = json.load(f)
    # Desktop clients nest under "installed"; web clients under "web".
    section = blob.get("installed") or blob.get("web") or blob
    return section["client_id"], section["client_secret"]


def main() -> int:
    if len(sys.argv) > 1:
        try:
            client_id, client_secret = from_json(sys.argv[1])
        except (OSError, KeyError, ValueError) as e:
            print(f"Could not read {sys.argv[1]}: {e}", file=sys.stderr)
            return 1
        print(f"Read the client from {sys.argv[1]}.\n")
    else:
        print("Paste the two values from the Google Cloud console, or re-run this with the")
        print("path to the JSON the console downloads:\n")
        print("    python3 scripts/google-oauth.py ~/Downloads/client_secret_*.json\n")
        print("They stay on this machine: the only thing they are sent to is Google.\n")
        client_id = input("Client ID:     ").strip()
        client_secret = input("Client secret: ").strip()
    if not client_id or not client_secret:
        print("\nBoth are needed. Nothing done.", file=sys.stderr)
        return 1

    port = free_port()
    redirect = f"http://127.0.0.1:{port}"
    state = secrets.token_urlsafe(16)

    url = AUTH + "?" + urllib.parse.urlencode({
        "client_id": client_id,
        "redirect_uri": redirect,
        "response_type": "code",
        "scope": SCOPE,
        "access_type": "offline",   # without this there is no refresh token at all
        "prompt": "consent",        # without this, only the first ever run returns one
        "state": state,
    })

    server = http.server.HTTPServer(("127.0.0.1", port), Catcher)
    threading.Thread(target=server.handle_request, daemon=True).start()

    print(f"\nOpening your browser. If nothing happens, visit this yourself:\n\n{url}\n")
    print('Google will warn that the app is unverified — that is expected for a personal')
    print('app. Choose "Advanced", then "Go to serana (unsafe)".\n')
    webbrowser.open(url)

    print("Waiting for the redirect...")
    server.socket.settimeout(300)
    while Catcher.code is None and Catcher.error is None:
        threading.Event().wait(0.2)

    if Catcher.error:
        print(f"\nGoogle refused: {Catcher.error}", file=sys.stderr)
        return 1
    if Catcher.state != state:
        # Somebody else's redirect reached our port; the code is not ours to trust.
        print("\nThe redirect did not match this request. Nothing done.", file=sys.stderr)
        return 1

    request = urllib.request.Request(
        TOKEN,
        data=urllib.parse.urlencode({
            "code": Catcher.code,
            "client_id": client_id,
            "client_secret": client_secret,
            "redirect_uri": redirect,
            "grant_type": "authorization_code",
        }).encode(),
        headers={"Content-Type": "application/x-www-form-urlencoded"},
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            payload = json.load(response)
    except urllib.error.HTTPError as e:
        detail = e.read().decode()
        print(f"\nGoogle rejected the exchange: {detail}", file=sys.stderr)
        if "client secret" in detail or "invalid_client" in detail:
            print(
                "\nThat is the secret, not the consent — the browser half worked. Re-run"
                "\nthis with the JSON the console gives you, which removes the paste:"
                "\n\n    python3 scripts/google-oauth.py ~/Downloads/client_secret_*.json",
                file=sys.stderr,
            )
        return 1

    refresh = payload.get("refresh_token")
    if not refresh:
        print(
            "\nNo refresh token came back. That happens when the app was authorised before"
            "\nwithout `prompt=consent`. Revoke it at"
            "\nhttps://myaccount.google.com/permissions and run this again.",
            file=sys.stderr,
        )
        return 1

    print("\nDone. Put these three lines in .env:\n")
    print(f"SERANA_GOOGLE_CLIENT_ID={client_id}")
    print(f"SERANA_GOOGLE_CLIENT_SECRET={client_secret}")
    print(f"SERANA_GOOGLE_REFRESH_TOKEN={refresh}")
    print("\n.env is gitignored, so none of this will be committed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
