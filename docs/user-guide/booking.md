# Booking

Pushin can run a local booking server so other people can reserve time from your real calendar availability. The public page is tunnel-ready: Pushin stays the source of truth, and a tunnel such as ngrok or Cloudflare Tunnel gives invitees a temporary public URL.

## How It Works

1. Open **Booking**.
2. Create or select an event type.
3. Start the local booking server.
4. Copy the local booking URL for private testing, or run a tunnel and paste its public base URL.
5. Send the generated public URL.

When someone books, Pushin creates a fixed calendar event and replans your tasks around it. If Google Calendar is connected, the booking syncs through Pushin's existing Google sync.

## Event Types

Each event type has a name, duration, buffer, color, active toggle, and private share token. Regenerate the token to invalidate an old link.

## Public Tunnels

The local server binds to `127.0.0.1` for safety. To make it public while Pushin is running, use one of these:

```sh
ngrok http 47610
```

```sh
cloudflared tunnel --url http://127.0.0.1:47610
```

If Pushin chooses a nearby fallback port, use the port shown in the Booking page.

## Current Limits

V1 collects only name and email. It does not send invitee email confirmations, collect payments, or provide an always-on hosted relay. The public link works while Pushin and the tunnel are both running.
