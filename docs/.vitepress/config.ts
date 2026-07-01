import { defineConfig } from "vitepress";

export default defineConfig({
  title: "Pushin Docs",
  description: "Documentation for Pushin, a local-first AI calendar and second brain.",
  base: "/Pushin/",
  cleanUrls: true,
  lastUpdated: true,
  head: [
    ["link", { rel: "icon", href: "/Pushin/pushin-icon.png" }],
    // The wide "Saira" wordmark that matches the app masthead.
    ["link", { rel: "preconnect", href: "https://fonts.googleapis.com" }],
    ["link", { rel: "preconnect", href: "https://fonts.gstatic.com", crossorigin: "" }],
    ["link", { rel: "stylesheet", href: "https://fonts.googleapis.com/css2?family=Saira:wght@500;600;700&display=swap" }],
    ["meta", { property: "og:type", content: "website" }],
    ["meta", { property: "og:title", content: "Pushin Docs" }],
    ["meta", { property: "og:description", content: "Local-first AI planning, calendar, and notes documentation." }],
  ],
  themeConfig: {
    logo: "/pushin-icon.png",
    siteTitle: "Pushin",
    search: {
      provider: "local",
    },
    nav: [
      { text: "Guide", link: "/getting-started/" },
      { text: "User Guide", link: "/user-guide/planning" },
      { text: "Developer Guide", link: "/developer-guide/architecture" },
      { text: "Releases", link: "https://github.com/Ilakkiyan/Pushin/releases" },
    ],
    sidebar: [
      {
        text: "Getting Started",
        items: [
          { text: "Overview", link: "/getting-started/" },
          { text: "Install Prerequisites", link: "/getting-started/prerequisites" },
          { text: "Run from Source", link: "/getting-started/run-from-source" },
          { text: "Download Releases", link: "/getting-started/releases" },
          { text: "AI Setup", link: "/getting-started/ai-setup" },
        ],
      },
      {
        text: "User Guide",
        items: [
          { text: "Planning with AI", link: "/user-guide/planning" },
          { text: "Daily Briefing & Action Bar", link: "/user-guide/briefing" },
          { text: "Calendar", link: "/user-guide/calendar" },
          { text: "Tasks and Projects", link: "/user-guide/tasks-projects" },
          { text: "Focus & Time-tracking", link: "/user-guide/focus" },
          { text: "Habits", link: "/user-guide/habits" },
          { text: "People", link: "/user-guide/people" },
          { text: "Meeting Companion", link: "/user-guide/meeting-companion" },
          { text: "Notes and Vault", link: "/user-guide/notes-vault" },
          { text: "Labels", link: "/user-guide/labels" },
          { text: "Quick Capture and Inbox", link: "/user-guide/inbox" },
          { text: "Booking", link: "/user-guide/booking" },
          { text: "Google Calendar Sync", link: "/user-guide/google-calendar" },
          { text: "Troubleshooting", link: "/user-guide/troubleshooting" },
        ],
      },
      {
        text: "Developer Guide",
        items: [
          { text: "Architecture", link: "/developer-guide/architecture" },
          { text: "Frontend", link: "/developer-guide/frontend" },
          { text: "Rust and Tauri", link: "/developer-guide/backend" },
          { text: "IPC Contract", link: "/developer-guide/ipc" },
          { text: "Parser and Scheduler", link: "/developer-guide/parser-scheduler" },
          { text: "Testing and Releases", link: "/developer-guide/testing-releases" },
        ],
      },
    ],
    socialLinks: [{ icon: "github", link: "https://github.com/Ilakkiyan/Pushin" }],
    footer: {
      message: "Local-first planning. No cloud account required.",
      copyright: "Released under the MIT License.",
    },
  },
});
