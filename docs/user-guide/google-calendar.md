# Google Calendar Sync

Pushin can sync with your primary Google Calendar.

## Privacy Model

There are no Pushin servers. Your computer talks directly to Google. You create and control your own Google OAuth client.

## Setup Outline

1. Create or select a Google Cloud project.
2. Enable the Google Calendar API.
3. Configure the OAuth consent screen.
4. Create a Desktop OAuth client.
5. Paste the Client ID and Client secret into Pushin settings.
6. Connect, approve access, and run the first sync.

## Why the Warning Appears

Google may show "Google hasn't verified this app" because the OAuth client is yours and not submitted for public verification. For personal use, click **Advanced** and continue to your app.

## Keeping Access

Move the OAuth app from Testing to Production in Google Cloud to avoid weekly refresh-token expiry.

For the full detailed walkthrough, see the Google Calendar section in the repository [README](https://github.com/Ilakkiyan/Pushin#google-calendar-sync-optional).
