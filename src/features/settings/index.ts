/**
 * Settings: the Settings window (⌘,, action `file.settings`) with built-in
 * pages (Appearance, System Settings, Keymap, Editor › General / Font / Code
 * Style / Files) plus pages contributed via `registerSettingsPage`, and the
 * runtime application of keymap / UI font size / high-contrast settings.
 *
 * On the desktop Settings open in a native child window (`activateSettingsWindow`
 * runs there), booted hidden after start-up and kept alive between opens; in
 * the browser build they open as an in-page dialog.
 *
 * `executeAction("file.settings", "keymap")` opens a specific page;
 * `file.newProjectsSettings` (IDEA's Settings for New Projects) edits the
 * global layer only, which new projects inherit.
 */
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { registerActions, type Action } from "@/platform/actions";
import { call, commands, isTauri } from "@/platform/ipc";
import { isPopupOpen, lazyComponent } from "@/ui";
import { loadSettings, useProject } from "@/platform/project";
import { disableRemote, restoreRemote } from "@/platform/remote";
import { provideSettingsSearchIndex, registerOverlay } from "@/workbench/contributions";
import { windowBackgroundHex } from "@/workbench/theme";
import { LazySettingsDialog, loadBuiltinPages, unloadBuiltinPages } from "./dialog/LazySettingsDialog";
import {
  closeSettings,
  openSettings,
  selectPage,
  setNewProjects,
  setQuery,
  settingsTitle,
  useSettingsDialog,
} from "./model/staging";
import { zoomActions } from "./appearanceRuntime";
import { activateFileTypes } from "./fileTypesRuntime";
import { trackRuntimeSettings } from "./runtime";
import { installActionProxies, installSettingsHost } from "./window/bridge";
import { AssignShortcutOverlay, showAssignShortcut } from "./keymap/AssignShortcut";
import { settingsSearchEntries } from "./searchIndex";
import { activatePythonStatus } from "./languages/PythonStatus";
import { isImeKey } from "@/platform/ime";

export { settingsWindowParent } from "./window/bridge";
export const SettingsWindow = lazyComponent(() => import("./window/SettingsWindow").then((m) => m.SettingsWindow));

/** Same event as `windows::LAUNCH_EVENT`: a pending launch (page) is waiting. */
const LAUNCH_EVENT = "openidea://launch";

/** Java's page first: JDK, Maven and the language server are what new projects most often need. */
const NEW_PROJECTS_PAGE = "languages.java";

function settingsActions(open: (page?: string, newProjects?: boolean) => void): Action[] {
  return [
    {
      id: "file.settings",
      title: "Settings…",
      category: "File",
      icon: "settings",
      keywords: ["preferences", "options", "configure"],
      run: ({ arg }) => open(typeof arg === "string" ? arg : undefined),
    },
    {
      id: "file.newProjectsSettings",
      title: "Settings for New Projects…",
      category: "File",
      icon: "settings",
      keywords: ["new projects setup", "default settings", "default project", "default jdk", "global settings"],
      run: ({ arg }) => open(typeof arg === "string" ? arg : NEW_PROJECTS_PAGE, true),
    },
    {
      id: "settings.keymap",
      title: "Keymap…",
      category: "File",
      icon: "keyboard",
      keywords: ["shortcuts", "keyboard", "bindings"],
      run: () => open("keymap"),
    },
    {
      id: "settings.appearance",
      title: "Appearance…",
      category: "View",
      icon: "sun",
      keywords: ["theme", "accent", "font size"],
      run: () => open("appearance"),
    },
  ];
}

/** Native Settings window when the backend can make one, else the dialog. */
async function showSettings(page?: string, newProjects = false) {
  if (isTauri()) {
    try {
      await call(commands.windowOpenSettings(page ?? null, newProjects, windowBackgroundHex()));
      return;
    } catch (e) {
      console.warn("settings window unavailable, using the dialog", e);
    }
  }
  await loadBuiltinPages();
  openSettings(page, { newProjects });
}

/**
 * Boots the Settings window hidden a bit after start-up: it's a whole second
 * webview, and booting it on ⌘, would keep Settings blank for a while.
 */
function prewarmSettingsWindow(): () => void {
  // hmm, 3 s keeps it out of the way of the project's own start-up (index, language servers)
  const timer = setTimeout(() => void commands.windowPrewarmSettings(windowBackgroundHex()).catch(() => {}), 3000);
  return () => clearTimeout(timer);
}

export function activate(): () => void {
  const disposers = [
    trackRuntimeSettings(),
    activatePythonStatus(),
    unloadBuiltinPages,
    registerOverlay({ id: "settings.dialog", component: LazySettingsDialog }),
    registerActions(zoomActions()),
    registerActions(settingsActions((page, newProjects) => void showSettings(page, newProjects))),
    activateFileTypes(),
    registerOverlay({ id: "settings.assignShortcut", component: AssignShortcutOverlay }),
    registerActions([
      {
        // Search Everywhere's ⌥↵ on an action (arg: the action id).
        id: "settings.assignShortcut",
        title: "Assign Shortcut…",
        category: "Settings",
        hidden: true,
        run: ({ arg }) => {
          if (typeof arg === "string") showAssignShortcut(arg);
        },
      },
    ]),
    provideSettingsSearchIndex(settingsSearchEntries),
    isTauri() ? installSettingsHost() : () => {},
    isTauri() ? prewarmSettingsWindow() : () => {},
  ];
  return () => disposers.reverse().forEach((d) => d());
}

function navigate(page: string | undefined) {
  if (!page) return;
  setQuery("");
  selectPage(page);
}

interface SettingsLaunch {
  page?: string;
  newProjects: boolean;
}

async function takeLaunch(): Promise<SettingsLaunch | null> {
  const launch = await commands.windowTakeLaunch().catch(() => null);
  return launch && { page: launch.settingsPage ?? undefined, newProjects: launch.newProjects };
}

/** A launch for the open window: its page, and the mode when it asks for new projects. */
function relaunch(launch: SettingsLaunch | null) {
  if (!launch) return;
  if (launch.newProjects) setNewProjects(true);
  navigate(launch.page);
}

function listenLaunch(onLaunch: () => void): () => void {
  let disposed = false;
  let off: (() => void) | undefined;
  void getCurrentWebviewWindow()
    .listen(LAUNCH_EVENT, onLaunch)
    .then((u) => {
      if (disposed) return u();
      off = u;
      // A page requested before we listened: its event is gone, the launch is not.
      onLaunch();
    });
  return () => {
    disposed = true;
    off?.();
  };
}

/** Esc and ⌘W cancel, like closing the window; ⌘, stays here. */
function installWindowKeys(): () => void {
  const onKey = (e: KeyboardEvent) => {
    if (e.defaultPrevented || isImeKey(e)) return;
    const mod = e.metaKey || e.ctrlKey;
    if ((e.key === "Escape" && !isPopupOpen()) || (mod && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "w")) {
      e.preventDefault();
      closeSettings();
    } else if (mod && e.key === ",") {
      e.preventDefault();
    }
  };
  window.addEventListener("keydown", onKey);
  return () => window.removeEventListener("keydown", onKey);
}

/** The OS close button (and Rust closing it on a project switch) cancels; the window only hides. */
function hideOnClose(): () => void {
  let disposed = false;
  let off: (() => void) | undefined;
  const win = getCurrentWebviewWindow();
  void win
    .onCloseRequested((e) => {
      e.preventDefault();
      if (useSettingsDialog.getState().open) closeSettings();
      else void win.hide().catch(() => {});
    })
    .then((u) => {
      if (disposed) return u();
      off = u;
    });
  return () => {
    disposed = true;
    off?.();
  };
}

/**
 * The parent's project and remote session. Read again on every reopen: the
 * hidden window outlives the parent switching projects.
 */
async function loadContext() {
  disableRemote();
  await restoreRemote();
  const project = await commands.projectCurrent().catch(() => null);
  if (project) useProject.setState({ project, settings: project.settings });
  else {
    useProject.setState({ project: null });
    await loadSettings().catch((e) => console.warn("cannot load settings", e));
  }
}

/**
 * The settings window (`settings-<parent>`): settings pages only, working on
 * the parent's project. It boots hidden ahead of time and stays alive: Cancel /
 * OK / Esc hide it, and the next launch reopens the dialog.
 */
export function activateSettingsWindow(parent: string): () => void {
  const pages = loadBuiltinPages();
  const launch = async () => {
    await pages;
    const next = await takeLaunch();
    if (!next) return;
    if (useSettingsDialog.getState().open) return relaunch(next);
    await loadContext();
    openSettings(next.page, { newProjects: next.newProjects });
  };
  const disposers = [
    trackRuntimeSettings(),
    unloadBuiltinPages,
    registerActions(settingsActions((page, newProjects) => relaunch({ page, newProjects: !!newProjects }))),
    installActionProxies(parent),
    listenLaunch(() => void launch()),
    installWindowKeys(),
    hideOnClose(),
    useSettingsDialog.subscribe((s, prev) => {
      if (prev.open && !s.open) void getCurrentWebviewWindow().hide().catch(() => {});
      if (s.newProjects !== prev.newProjects) {
        void getCurrentWebviewWindow()
          .setTitle(settingsTitle(s.newProjects))
          .catch(() => {});
      }
    }),
  ];
  // ok so a prewarmed window has no launch and stays closed; `listenLaunch` may also get here first
  void launch();
  return () => disposers.reverse().forEach((d) => d());
}
