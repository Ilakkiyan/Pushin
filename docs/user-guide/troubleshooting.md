# Troubleshooting

## The App Builds but Does Not Open

Check that Tauri prerequisites are installed for your OS. On Linux, missing WebKitGTK packages are the most common cause.

## The AI Is Not Ready

Confirm that:

- the model has downloaded
- the local server URL points to the running server
- no other process is using port `8080`

## Google Sync Fails

Common causes:

- Calendar API was not enabled
- the Google account was not added as a test user while the OAuth app is in Testing
- the OAuth client type is not Desktop app
- the app is still in Testing and the refresh token expired

## Calendar Looks Wrong After an Edit

Run **Reschedule** or make another scheduling change. Locked task blocks stay fixed while other work is re-planned around them.
