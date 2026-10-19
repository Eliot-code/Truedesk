---
title: Phones and tablets
description: Control a computer from an Android phone or tablet with the Truedesk app, or from any phone's browser, and share an Android device so someone can control it.
---

You can control a computer running Truedesk from a phone or tablet. The
computer still needs the Truedesk app to host.

## Android

Install the Truedesk app for Android:

1. Download `Truedesk_<version>_android.apk` from the
   [releases page](https://github.com/Eliot-code/truedesk/releases), or the
   newest build from the **Android app** workflow's artifacts.
2. Open it on the phone. Android asks you to allow installing apps from your
   browser or file manager the first time.
3. Open Truedesk, check the **Relay** under the connect form (it has to be the
   relay your computers use), and connect with the computer's ID and password.

The app is the [browser viewer](/docs/browser-viewer/) in an app window, so
it can do what the browser viewer can: screen, mouse and keyboard, sound,
files and clipboard, over the relay.

## Share an Android device

The Android app can also be shared, so a Truedesk viewer sees and controls
*this* phone or tablet — remote support for a phone.

1. On the connect form, tap **Share this device**.
2. Turn on **Truedesk remote control** when asked: this is an Android
   accessibility service, which lets the app carry out the viewer's taps and
   typing. You turn it on in **Settings → Accessibility**; turn it off there
   any time.
3. Tap **Start sharing** and allow the system's screen-capture prompt. A
   notification stays up for as long as you share.
4. Give the **device ID** and **password** on the screen to the person
   connecting. When they connect you get an **Allow / Deny** prompt, unless
   you share without asking.

The viewer's mouse becomes touches: a click taps, a drag drags, the wheel and
two-finger drags scroll, and right-click is Back. Typing goes to the focused
field, and the arrows, Home, End and Esc map to editing and the Back button.

Only Android can be shared this way. iPhone and iPad can be viewed from
Truedesk but not controlled, because iOS does not let an app inject input.

## iPhone and iPad

Open your Truedesk website's `/connect/` page in Safari and connect from
there. To keep it a tap away, use **Share → Add to Home Screen**. An iOS app
is planned; it needs an Apple developer account to install outside the
App Store.

## Touch controls

| Gesture | What it does |
| --- | --- |
| Tap | Click. |
| Hold one finger still for half a second | Right-click. |
| Drag one finger | Drag with the left button held: select text, move windows. |
| Drag two fingers | Scroll. |
| Pinch | Zoom the picture in or out on your side. Pinch back to fit to show the whole screen again. |

## Typing

Tap **Keyboard** in the toolbar to bring up the phone's keyboard. What you
type goes straight to the other computer, autocorrect and suggestions
included. A row of keys appears above the screen for what phone keyboards
lack: **Esc**, **Tab** and the arrows. **Ctrl**, **Alt** and **Win** hold
for the next key, so **Ctrl** then **c** copies.
