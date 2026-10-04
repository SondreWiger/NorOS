# NorOS — Architecture & Roadmap

*Draft for approval · 2026-10-04*

> **NorOS** — a fast, unbloated, privacy-first operating system with the polish of a Mac and none of the restrictions. You own the machine. Nothing leaves it unless you say so.

---

## 1. Principles

These decide every trade-off. When two goals conflict, the higher one wins.

1. **The user is in control.** No hidden behavior, no forced defaults, no "you're not allowed to". Every built-in component can be replaced.
2. **Nothing leaves the machine without need and consent.** No telemetry, no accounts, no cloud, no AI. Network activity is visible and controllable per app.
3. **Fast and lean.** Every component has a memory and startup budget. Nothing runs that the user didn't ask for.
4. **Polished by default.** Out of the box it looks and feels as refined as macOS.
5. **Runs what people need.** Linux apps, Windows `.exe`, browsers and games.

---

## 2. The big picture

```
┌──────────────────────────────────────────────────────────────┐
│  APPS        Files · Terminal · Settings · Privacy Center ·  │
│              Editor · Media · Browser · Store · Theme Studio │
├──────────────────────────────────────────────────────────────┤
│  COMPAT      Native apps · Flatpak · .deb · .exe (Wine/Proton)│
├──────────────────────────────────────────────────────────────┤
│  SHELL       Menu bar · Dock · Launcher · Notifications ·    │
│  (modules)   Overview · Lock screen   ← every piece swappable│
├──────────────────────────────────────────────────────────────┤
│  FJORD       Our Wayland compositor & window manager (Rust)  │
├──────────────────────────────────────────────────────────────┤
│  VAKT        Privacy daemon: per-app firewall, permissions,  │
│              sandbox, activity ledger                        │
├──────────────────────────────────────────────────────────────┤
│  BASE        Invisible Debian 13 userland · atomic A/B       │
│              system image · LUKS2 encryption                 │
├──────────────────────────────────────────────────────────────┤
│  KERNEL      Linux (now)  ───────────►  BERG, our Rust kernel│
│                                         (long-term track)    │
└──────────────────────────────────────────────────────────────┘
        Architectures: x86_64 (PCs) · ARM64 (VMs, later ARM hw)
```

Everything from **Vakt** upward is ours. The layers below are standard foundations we shape and hide, and which we can replace later.

---

## 3. Components

| Code name | What it is | Built with |
|---|---|---|
| **Fjord** | Compositor and window manager: windows, animations, snapping, workspaces, overview | Rust + Smithay |
| **Shell** | Menu bar, dock, launcher (search), notifications, control center, lock screen. Each one is a separate **module** | Rust + GTK4 (layer-shell) |
| **Lys** | Theme engine: real **CSS** for every UI element, plus layout config (where the bar, dock and buttons go) | GTK4 CSS + TOML |
| **Theme Studio** | Visual theme editor: click an element, change it, see it live, export the theme | Rust + GTK4 |
| **Vakt** | Privacy daemon: per-app network firewall, permission prompts (camera, mic, files, location), the live "who is sending what" feed, and an **opt-in, encrypted** local activity history | Rust, eBPF/nftables, bubblewrap |
| **Privacy Center** | The app on top of Vakt: see everything, switch anything off | Rust + GTK4 |
| **Bro** | Bridges for running apps: `.deb` (container layer), `.exe` (Wine/Proton with automatic per-app prefixes), Flatpak/Flathub | Rust glue around existing tools |
| **Store** | NorOS app store for apps, themes and extensions. Works offline with local packages; browsing it is the only time it goes online | Rust + GTK4 |
| **Nøkkel** | Offline license: an Ed25519-signed key, verified locally, never phones home | Rust |
| **Berg** | Our own kernel. Long-term goal: **Linux-ABI compatible**, so it can replace Linux without breaking any app | Rust, bare metal |

**Built-in apps:** Files, Terminal, Settings, Privacy Center, Text Editor, Media Player, Browser (our own, on a privacy-tuned engine; Firefox available as an alternative), Store and Theme Studio.

### Why these choices
- **GTK4 for the UI** because it is styled with real CSS natively. That makes "custom CSS for everything" a core feature rather than an add-on, and it is fast, accessible and mature.
- **Debian underneath** gives native `.deb` support and the compatibility Steam, Proton and Wine depend on. Users never see it.
- **Atomic A/B system images** mean the core system is read-only and updates apply all at once. A failed update rolls back automatically, and you can boot the previous version at any time. Your apps and files are kept separately and are never touched.
- **A Linux-ABI-compatible Berg kernel** makes the "its own thing" goal achievable: the day Berg is ready, it can swap in underneath without breaking a single app.

---

## 4. Privacy model

- **Zero telemetry.** No crash reporting, analytics or accounts. Crash logs stay local, and you choose whether to share one.
- **Network calls the system itself makes** (update checks, store browsing) are opt-in, listed in Privacy Center, and can be switched off. Updates can also be installed from a file.
- **Per-app firewall:** the first time an app tries to reach the network, you're asked. All traffic is shown per app, live.
- **Sandboxed apps:** apps from the store run in sandboxes, and permissions are granted and revocable in Privacy Center.
- **Activity history:** off by default. When enabled it is stored encrypted on the machine, viewable and exportable, and can be wiped with one click.
- **Full-disk encryption** (LUKS2) is on by default at install.

---

## 5. Customization model

Everything visible is a **module** with a manifest. Users can:

1. **Theme:** colors, fonts, corner radius, blur, animations, either in Settings or as raw CSS.
2. **Layout:** menu bar top, bottom or off; dock on any edge or off; window buttons left or right with any style; mix Mac elements in or leave them out.
3. **Swap components:** replace the dock, launcher or even the whole shell with another module.
4. **Extend:** sandboxed extensions with a documented SDK, distributed through the Store or installed from a file.
5. **Modes:** presets for different uses (Work, Gaming, Minimal, Kiosk) that switch modules and settings at once.

---

## 6. How it gets built

- **Repo:** `SondreWiger/NorOS` (private).
- **Builds run in GitHub Actions**, not on the Mac. Every push builds images for x86_64 and ARM64.
- **Automated boot test:** CI boots each image in QEMU and saves **screenshots**, so every version is verified to boot to the desktop.
- **Outputs:** a bootable `.iso` (live and installer) and a VM disk image (`.qcow2`).
- **Website:** a separate public repo, hosted free on GitHub Pages until NorOS gets a domain.

---

## 7. Roadmap

| Version | Name | Goal |
|---|---|---|
| **0.1** | *Første lys* (First light) | Bootable image for both architectures. Boots into **Fjord** with wallpaper, menu bar, dock, launcher and a terminal. Builds and boot tests run in CI. |
| 0.2 | *Form* | Settings app, **Lys** theme engine (CSS, colors, layout toggles), Files and Text Editor. |
| 0.3 | *Fundament* | Graphical installer, full-disk encryption, atomic A/B updates with rollback. |
| 0.4 | *Vakt* | Privacy Center, per-app firewall, permission prompts, sandboxing, activity history. |
| 0.5 | *Bro* | App compatibility: Flatpak, `.deb`, `.exe` through Wine/Proton, Steam. |
| 0.6 | *Verksted* (Workshop) | Module and extension SDK, Theme Studio, Modes. |
| 0.7 | *Hverdag* (Everyday) | Browser, Media Player, notifications, polish and animations. |
| 0.8 | *Jern* (Iron) | Real x86 hardware: GPU drivers, Wi-Fi, sleep and battery, testing on your PC. |
| 1.0 | *NorOS* | Store, offline licensing, website launch, sales. |

**Parallel tracks**
- **Website**, starting now: brutalist, hard, unique.
- **Berg kernel**, starting after 0.2: boot in QEMU → memory → scheduler → filesystem → Linux syscalls, one step at a time.

---

## 8. Honest risks

- **Scale.** This is a very large project. The roadmap delivers something usable at every step, so it's never all-or-nothing.
- **Games and `.exe`** depend on Wine/Proton. Most things work, but not everything, especially anti-cheat games.
- **ARM64 hardware.** M3/M4 Macs can't boot NorOS natively (only in a VM), so x86 PCs are the main real-hardware target.
- **Hardware drivers** come from Linux. That is a big advantage now, and the main challenge for the Berg kernel later.
- **Licenses:** our own code can stay closed. Changes we make to Linux, Wine or other GPL/LGPL parts must be published. That's normal and manageable.
- **Offline licenses can be cracked.** We accept that as the price of privacy.
- **Free CI minutes** may run out. If so, the options are GitHub Pro (~$4/month) or making the repo public.
