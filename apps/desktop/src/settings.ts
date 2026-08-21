import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { creationApi } from "./creation/api";
import { loadComposerPack } from "./creation/composer-pack";
import {
  exportComposerPng,
  renderComposerRecipe,
  type ComposerRenderPorts,
} from "./creation/composer-renderer";
import type { PetCatalogEntry } from "./pets/pet-catalog-contract";
import { createPetProfileClient } from "./pets/pet-profile-contract";
import { loadPreferences, requestPetDisplayScale } from "./runtime/bridge";
import { isProbePreferences } from "./runtime/contracts";
import {
  PET_CALIBRATION_PREVIEW_REQUEST,
  PET_CALIBRATION_PREVIEW_RESULT,
} from "./runtime/contracts";
import {
  type PetCalibrationV1,
} from "./runtime/pet-calibration";
import { requestPetCalibrationPreview } from "./runtime/pet-stage";
import {
  AdoptionCreationView,
  type AdoptionCreationElements,
} from "./settings/adoption-creation-view";
import { CandidatePreviewController } from "./settings/candidate-dynamic-preview";
import {
  ComposerCreationView,
  type ComposerCreationElements,
} from "./settings/composer-creation-view";
import {
  CreationPageActivity,
  CreationPageFocusManager,
  CreationPageRun,
  type CreationRoute,
  type DraftChoice,
} from "./settings/creation-page-run";
import { finalizeCreation } from "./settings/creation-finalizer";
import {
  DisplaySizeControl,
  initializeDisplaySizeControl,
} from "./settings/display-size-control";
import {
  catalogSwitchStatus,
  deleteCurrentCatalogPet,
  mergeCatalogWarnings,
} from "./settings/pet-catalog-delete-flow";
import { buildPetListRows, type PetListAction } from "./settings/pet-catalog-view-model";
import { PetProfileEditor } from "./settings/pet-profile-editor";
import {
  PetCalibrationCatalogCoordinator,
  PetCalibrationControl,
} from "./settings/pet-calibration-control";
import { initializeSettingsNavigation } from "./settings/settings-navigation";
import { wireSettingsPageLifecycle } from "./settings/settings-page-lifecycle";
import { SettingsCloseCoordinator } from "./settings/settings-close-coordinator";
import { requestPetSwitch } from "./settings/pet-switch-client";
import {
  createPhotoAvatarCreationDomPorts,
  PhotoAvatarCreationView,
} from "./settings/photo-avatar-creation-view";

interface DeleteOutcome { warning: string | null; }

const $ = <T extends HTMLElement>(id: string): T => {
  const element = document.getElementById(id);
  if (!element) throw new Error(`missing element #${id}`);
  return element as T;
};

const listEl = $<HTMLDivElement>("list");
const statusEl = $<HTMLDivElement>("status");
const tabList = $<HTMLButtonElement>("tab-list");
const tabCreate = $<HTMLButtonElement>("tab-create");
const viewList = $<HTMLDivElement>("view-list");
const viewCreate = $<HTMLDivElement>("view-create");
const creationHome = $<HTMLElement>("creation-home");
const uploadWorkspace = $<HTMLElement>("photo-avatar-workspace");
const composerWorkspace = $<HTMLElement>("composer-creation-workspace");
const adoptionWorkspace = $<HTMLElement>("adoption-creation-workspace");
const creationStatus = $<HTMLDivElement>("creation-page-status");
const routeButtons = Array.from(
  document.querySelectorAll<HTMLButtonElement>("[data-creation-route]"),
);
const workspaceBackButtons = Array.from(
  document.querySelectorAll<HTMLButtonElement>(".workspace-back"),
);
const displaySizeSlider = $<HTMLInputElement>("display-size-slider");
const displaySizeOutput = $<HTMLOutputElement>("display-size-output");
const displaySizeStatus = $<HTMLElement>("display-size-status");
const displaySizeError = $<HTMLElement>("display-size-error");
const displaySizePresets = Array.from(
  document.querySelectorAll<HTMLButtonElement>("[data-display-scale]"),
);
const calibrationSection = $<HTMLElement>("calibration-section");
const calibrationPetName = $<HTMLElement>("calibration-pet-name");

const calibrationControl = new PetCalibrationControl({
  elements: {
    root: calibrationSection,
    petName: calibrationPetName,
    breath: $<HTMLInputElement>("calibration-breath"),
    breathOutput: $<HTMLOutputElement>("calibration-breath-output"),
    feedback: $<HTMLInputElement>("calibration-feedback"),
    feedbackOutput: $<HTMLOutputElement>("calibration-feedback-output"),
    reset: $<HTMLButtonElement>("calibration-reset"),
    feedbackTest: $<HTMLButtonElement>("calibration-feedback-test"),
    cancel: $<HTMLButtonElement>("calibration-cancel"),
    save: $<HTMLButtonElement>("calibration-save"),
    status: $<HTMLElement>("calibration-status"),
    error: $<HTMLElement>("calibration-error"),
  },
  ports: {
    load: (petId) => invoke<PetCalibrationV1>("pet_calibration_load", { petId }),
    save: (petId, value) => invoke<PetCalibrationV1>("pet_calibration_save", { petId, value }),
    runtime: async (petId, action, value) => {
      const result = await requestPetCalibrationPreview(petId, action, value, {
        ports: {
          listen: async (handler) => listen<unknown>(
            PET_CALIBRATION_PREVIEW_RESULT,
            ({ payload }) => handler(payload),
          ),
          emit: (request) => emitTo("pet", PET_CALIBRATION_PREVIEW_REQUEST, request),
        },
      });
      if (!result.ok) throw new Error(result.message);
    },
  },
});
calibrationControl.mount();
const calibrationCatalog = new PetCalibrationCatalogCoordinator(calibrationControl);

let catalogEntries: PetCatalogEntry[] = [];
let catalogBusy: "switch" | "delete" | null = null;
let selectedView: "list" | "create" = "list";
let activeCreationRoute: CreationRoute | null = null;
const creationBusy = { router: false, adoption: false, activity: false };

const profileEditor = new PetProfileEditor(createPetProfileClient(), {
  elements: {
    root: $<HTMLElement>("pet-profile-editor"),
    form: $<HTMLFormElement>("pet-profile-form"),
    name: $<HTMLInputElement>("pet-profile-name"),
    gender: $<HTMLSelectElement>("pet-profile-gender"),
    birthDate: $<HTMLInputElement>("pet-profile-birth-date"),
    cancel: $<HTMLButtonElement>("pet-profile-cancel"),
    save: $<HTMLButtonElement>("pet-profile-save"),
    loading: $<HTMLElement>("pet-profile-loading"),
    error: $<HTMLElement>("pet-profile-error"),
  },
  refreshCatalog: () => renderList(),
  setStatus: setCatalogStatus,
});

const creationActivity = new CreationPageActivity(
  (busy) => setCreationBusy("activity", busy),
);
const creationFocus = new CreationPageFocusManager();

function setCatalogStatus(message: string, tone: "info" | "error" | "warning" = "info"): void {
  statusEl.textContent = message;
  statusEl.classList.toggle("error", tone === "error");
  statusEl.classList.toggle("warning", tone === "warning");
}

function renderCatalogRows(): void {
  listEl.replaceChildren();
  if (catalogEntries.length === 0) {
    profileEditor.reconcileAnchor(null, null);
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.textContent = "还没有宠物。前往“创建宠物”上传一张猫咪照片吧。";
    listEl.append(empty);
    return;
  }
  let editorAnchor: HTMLElement | null = null;
  let editorOpener: HTMLButtonElement | null = null;
  for (const row of buildPetListRows(catalogEntries)) {
    const entry = catalogEntries.find((item) => item.petId === row.petId);
    const item = document.createElement("div");
    item.className = entry?.isCurrent ? "pet-item active" : "pet-item";
    const copy = document.createElement("div");
    copy.className = "pet-copy";
    const heading = document.createElement("div");
    heading.className = "pet-heading";
    const name = document.createElement("span");
    name.className = "pet-name";
    name.textContent = row.title;
    heading.append(name);
    if (row.badge) {
      const badge = document.createElement("span");
      badge.className = "pet-badge";
      badge.textContent = row.badge;
      heading.append(badge);
    }
    const detail = document.createElement("p");
    detail.className = "pet-detail";
    detail.textContent = row.detail;
    copy.append(heading, detail);
    const actions = document.createElement("div");
    actions.className = "actions";
    for (const action of row.actions) {
      const button = document.createElement("button");
      button.className = action.kind === "switch" ? "primary" : action.kind === "delete" ? "danger" : "";
      button.disabled = catalogBusy !== null;
      button.textContent = catalogBusy === action.kind
        ? action.kind === "switch" ? "正在切换…" : "正在删除…"
        : action.label;
      button.addEventListener("click", () => {
        void handleCatalogAction(row.petId, action, item, button);
      });
      if (action.kind === "edit" && row.petId === profileEditor.editingPetId) {
        editorAnchor = item;
        editorOpener = button;
      }
      actions.append(button);
    }
    item.append(copy, actions);
    listEl.append(item);
  }
  profileEditor.reconcileAnchor(editorAnchor, editorOpener);
}

async function renderList(): Promise<boolean> {
  try {
    catalogEntries = await invoke<PetCatalogEntry[]>("pet_catalog_list");
    renderCatalogRows();
    await calibrationCatalog.reconcile(catalogEntries);
    return true;
  } catch (error) {
    calibrationCatalog.unavailable("当前宠物目录不可用，无法安全校准。请稍后重试。");
    setCatalogStatus(`读取宠物目录失败。请确认桌面宠物正在运行后重试：${String(error)}`, "error");
    return false;
  }
}

async function handleCatalogAction(
  petId: string,
  action: PetListAction,
  item: HTMLElement,
  button: HTMLButtonElement,
): Promise<void> {
  if (action.kind === "edit") {
    await profileEditor.open(petId, item, button);
    return;
  }
  if (action.kind === "continue") {
    switchView("create");
    return;
  }
  if (catalogBusy) return;
  if (action.kind === "switch") {
    await switchCatalogPet(petId);
    return;
  }
  await deleteCatalogPet(petId);
}

async function switchCatalogPet(petId: string): Promise<void> {
  catalogBusy = "switch";
  renderCatalogRows();
  try {
    const result = await requestPetSwitch(petId);
    if (!result.ok) {
      setCatalogStatus(`切换失败。请确认宠物窗口可用后重试：${result.message}`, "error");
      return;
    }
    if (await renderList()) {
      const status = catalogSwitchStatus(result.warning);
      setCatalogStatus(status.message, status.tone);
    }
  } catch (error) {
    setCatalogStatus(`切换失败。请稍后重试：${String(error)}`, "error");
  } finally {
    catalogBusy = null;
    renderCatalogRows();
  }
}

async function deleteCatalogPet(petId: string): Promise<void> {
  const entry = catalogEntries.find((item) => item.petId === petId);
  if (!entry || !window.confirm("确定删除这只宠物吗？此操作会移除它的本地资料和生成任务。")) return;
  catalogBusy = "delete";
  renderCatalogRows();
  try {
    let outcome: DeleteOutcome;
    let switchWarning: string | undefined;
    if (entry.isCurrent) {
      const result = await deleteCurrentCatalogPet({
        switchToBuiltin: async () => {
          const switched = await requestPetSwitch("pet-live2d-v1");
          return switched.ok
            ? { ok: true, ...(switched.warning ? { warning: switched.warning } : {}) }
            : { ok: false, message: switched.message };
        },
        remove: () => invoke<DeleteOutcome>("pet_delete_full", { petId }),
        refresh: async () => { await renderList(); },
      });
      if (result.kind === "switchFailed") {
        setCatalogStatus(`无法切换至默认猫，因此未删除当前宠物。请确认宠物窗口可用后重试：${result.message}`, "error");
        return;
      }
      if (result.kind === "deleteFailed") throw result.error;
      outcome = result.outcome;
      switchWarning = result.switchWarning;
    } else {
      outcome = await invoke<DeleteOutcome>("pet_delete_full", { petId });
    }
    if (await renderList()) {
      const warning = mergeCatalogWarnings(switchWarning, outcome.warning);
      setCatalogStatus(warning ?? "宠物已删除。", warning ? "warning" : "info");
    }
  } catch (error) {
    setCatalogStatus(`删除失败。请稍后重试：${String(error)}`, "error");
  } finally {
    catalogBusy = null;
    renderCatalogRows();
  }
}

const photoAvatarView = new PhotoAvatarCreationView(
  {
    api: creationApi,
    finalize: finalizeCreation,
  },
  createPhotoAvatarCreationDomPorts(document, { showThirdPartyConsent }, () => showCreationHome()),
);

const composerRoot = "/creation-content/composer/cat-cute-v1";
const composerAssetUrl = (relativePath: string): string =>
  `${composerRoot}/${encodeRelativeAssetPath(relativePath)}`;
const composerRenderPorts: ComposerRenderPorts = {
  createSurface: (width, height) => {
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    return canvas;
  },
  context: (surface) => {
    const context = surface.getContext("2d");
    if (!context) throw new Error("当前环境无法创建猫咪组合画布");
    return context;
  },
  loadImage: (url) => new Promise<HTMLImageElement>((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error(`组合素材未能加载：${url}`));
    image.src = url;
  }),
  assetUrl: composerAssetUrl,
  toPng: (surface) => new Promise<Blob>((resolve, reject) => {
    surface.toBlob((blob) => {
      if (blob) resolve(blob);
      else reject(new Error("组合 PNG 导出失败"));
    }, "image/png");
  }),
};

const composerPreview = new CandidatePreviewController();
const composerView = new ComposerCreationView({
  creation: creationApi,
  loadPack: () => loadComposerPack(`${composerRoot}/manifest.json`),
  render: async (pack, recipe, target) => {
    if (!target) throw new Error("组合预览画布尚未准备好");
    await renderComposerRecipe(pack, recipe, target, composerRenderPorts);
  },
  exportPng: (pack, recipe) => exportComposerPng(pack, recipe, composerRenderPorts),
  blobToBase64: readBlobBase64,
  assetAvailable: async (relativePath) => {
    const response = await fetch(composerAssetUrl(relativePath), { cache: "force-cache" });
    return response.ok;
  },
  preview: composerPreview,
  finalize: finalizeCreation,
  confirm: (message) => window.confirm(message),
  activity: creationActivity,
});

const adoptionPreview = new CandidatePreviewController();
const adoptionElements = queryAdoptionCreationElements();
const adoptionView = new AdoptionCreationView({
  creation: {
    adoptionCatalog: creationApi.adoptionCatalog,
    adoptionStart: creationApi.adoptionStart,
    snapshot: creationApi.snapshot,
    recoverFinalization: creationApi.recoverFinalization,
  },
  previewRoot: adoptionElements.root.querySelector<HTMLElement>("#adoption-preview")!,
  preview: adoptionPreview,
  assetUrl: (templateId, relativePath) =>
    `/creation-content/adoption/${encodeURIComponent(templateId)}/${encodeRelativeAssetPath(relativePath)}`,
  loadMotionProfile: async (url) => {
    const response = await fetch(url, { cache: "force-cache" });
    if (!response.ok) throw new Error(`动态参数请求失败（HTTP ${response.status}）`);
    return response.json() as Promise<unknown>;
  },
  finalize: finalizeCreation,
  switchPet: requestPetSwitch,
  refreshPets: async () => { await renderList(); },
  onBusyChange: (busy) => setCreationBusy("adoption", busy),
  activity: creationActivity,
  onBack: showCreationHome,
});
adoptionView.mount(adoptionElements);

const composerElements = queryComposerCreationElements();
const creationPage = new CreationPageRun({
  creation: creationApi,
  views: {
    upload: {
      open: async (sessionId) => { await photoAvatarView.enter(sessionId); },
      leave: () => photoAvatarView.leave(),
    },
    composer: {
      open: async (sessionId) => {
        composerView.mount(composerElements);
        if (sessionId) await composerView.restore(sessionId);
        else await composerView.open();
      },
      leave: () => composerView.destroy(),
    },
    adoption: {
      open: async () => { await adoptionView.open(); },
      leave: () => adoptionView.leave(),
    },
  },
  dialog: { showDraftChoice },
  onRoute: showRouteWorkspace,
  onBusy: (busy) => setCreationBusy("router", busy),
  activity: creationActivity,
});

function setCreationBusy(source: keyof typeof creationBusy, busy: boolean): void {
  creationBusy[source] = busy;
  const pageBusy = creationBusy.router || creationBusy.adoption || creationBusy.activity;
  viewCreate.setAttribute("aria-busy", String(pageBusy));
  for (const button of [...routeButtons, ...workspaceBackButtons, tabList, tabCreate]) {
    button.disabled = pageBusy;
    button.setAttribute("aria-disabled", String(pageBusy));
  }
  for (const workspace of [uploadWorkspace, composerWorkspace, adoptionWorkspace]) {
    workspace.inert = pageBusy;
    workspace.setAttribute("aria-busy", String(pageBusy));
    for (const control of workspace.querySelectorAll<HTMLElement>("button, input, [tabindex]")) {
      const nativeDisabled = "disabled" in control && Boolean(
        (control as HTMLButtonElement | HTMLInputElement).disabled,
      );
      control.setAttribute("aria-disabled", String(pageBusy || nativeDisabled));
    }
  }
}

function setCreationStatus(message: string, error = false): void {
  creationStatus.textContent = message;
  creationStatus.classList.toggle("error", error);
}

function showRouteWorkspace(route: CreationRoute): void {
  activeCreationRoute = route;
  creationHome.hidden = true;
  uploadWorkspace.hidden = route !== "upload";
  composerWorkspace.hidden = route !== "composer";
  adoptionWorkspace.hidden = route !== "adoption";
  setCreationStatus("");
  creationFocus.enter(route, workspaceForRoute(route));
}

function showCreationHome(returnRoute: CreationRoute | null = activeCreationRoute): void {
  creationPage.close();
  activeCreationRoute = null;
  creationHome.hidden = false;
  uploadWorkspace.hidden = true;
  composerWorkspace.hidden = true;
  adoptionWorkspace.hidden = true;
  if (returnRoute) creationFocus.returnToTrigger(returnRoute);
}

function showDraftChoice(method: "upload" | "composer"): Promise<DraftChoice> {
  const dialog = $<HTMLDialogElement>("draft-choice-dialog");
  $<HTMLElement>("draft-choice-description").textContent = method === "composer"
    ? "当前有一份引导组合草稿安全保存在本机。可以继续组合，或放弃后再打开刚选择的方式。"
    : "当前有一份上传创建草稿安全保存在本机。可以继续上传创建，或放弃后再打开刚选择的方式。";
  return new Promise((resolve) => {
    const settle = () => {
      const choice = dialog.returnValue;
      resolve(choice === "continue" || choice === "abandon" ? choice : "cancel");
    };
    dialog.addEventListener("close", settle, { once: true });
    dialog.showModal();
  });
}

function showThirdPartyConsent(): Promise<boolean> {
  const dialog = $<HTMLDialogElement>("photo-avatar-consent-dialog");
  return new Promise((resolve) => {
    dialog.addEventListener("close", () => resolve(dialog.returnValue === "accept"), { once: true });
    dialog.showModal();
  });
}

for (const button of routeButtons) {
  button.addEventListener("click", () => {
    const route = button.dataset.creationRoute;
    if (route !== "upload" && route !== "composer" && route !== "adoption") return;
    creationFocus.remember(route, button);
    void creationPage.open(route).catch((error) => {
      showCreationHome(route);
      setCreationStatus(`创建入口暂时无法打开：${String(error)}`, true);
    });
  });
}
for (const button of workspaceBackButtons) {
  if (button.id === "adoption-back") continue;
  button.addEventListener("click", () => showCreationHome());
}

function queryComposerCreationElements(): ComposerCreationElements {
  const query = <T extends Element>(selector: string): T => {
    const element = composerWorkspace.querySelector<T>(selector);
    if (!element) throw new Error(`missing composer element ${selector}`);
    return element;
  };
  return {
    canvas: query<HTMLCanvasElement>("[data-composer-canvas]"),
    steps: query<HTMLElement>("[data-composer-steps]"),
    options: query<HTMLElement>("[data-composer-options]"),
    saveStatus: query<HTMLElement>("[data-composer-save]"),
    message: query<HTMLElement>("[data-composer-message]"),
    previousButton: query<HTMLButtonElement>("[data-composer-previous]"),
    nextButton: query<HTMLButtonElement>("[data-composer-next]"),
    candidateButton: query<HTMLButtonElement>("[data-composer-candidate]"),
    candidatePreview: query<HTMLElement>("[data-composer-dynamic]"),
    nameInput: query<HTMLInputElement>("[data-composer-name]"),
    finishButton: query<HTMLButtonElement>("[data-composer-finish]"),
    abandonButton: query<HTMLButtonElement>("[data-composer-abandon]"),
  };
}

function queryAdoptionCreationElements(): AdoptionCreationElements {
  return {
    root: $<HTMLElement>("adoption-root"),
    catalog: $<HTMLElement>("adoption-catalog"),
    selectedName: $<HTMLElement>("adoption-selected-name"),
    selectedPersonality: $<HTMLElement>("adoption-selected-personality"),
    nameInput: $<HTMLInputElement>("adoption-pet-name"),
    actionButton: $<HTMLButtonElement>("adoption-action"),
    refreshButton: $<HTMLButtonElement>("adoption-refresh"),
    backButton: $<HTMLButtonElement>("adoption-back"),
    status: $<HTMLElement>("adoption-status"),
  };
}

function workspaceForRoute(route: CreationRoute): HTMLElement {
  if (route === "upload") return uploadWorkspace;
  if (route === "composer") return composerWorkspace;
  return adoptionWorkspace;
}

function encodeRelativeAssetPath(relativePath: string): string {
  const parts = relativePath.split("/");
  if (parts.some((part) => !part || part === "." || part === ".." || part.includes("\\"))) {
    throw new Error("素材路径无效");
  }
  return parts.map((part) => encodeURIComponent(part)).join("/");
}

function readBlobBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error("无法读取组合图片"));
    reader.onload = () => {
      const dataUrl = String(reader.result);
      const comma = dataUrl.indexOf(",");
      if (comma < 0) reject(new Error("组合图片编码无效"));
      else resolve(dataUrl.slice(comma + 1));
    };
    reader.readAsDataURL(blob);
  });
}

function switchView(view: "list" | "create"): void {
  if (selectedView === view) return;
  selectedView = view;
  viewList.hidden = view !== "list";
  viewCreate.hidden = view !== "create";
  tabList.classList.toggle("active", view === "list");
  tabCreate.classList.toggle("active", view === "create");
  tabList.setAttribute("aria-selected", String(view === "list"));
  tabCreate.setAttribute("aria-selected", String(view === "create"));
  if (view === "list") {
    creationPage.close();
    activeCreationRoute = null;
    creationFocus.cancel();
    void renderList();
  } else {
    profileEditor.reconcileAnchor(null, null);
    showCreationHome();
  }
}

tabList.addEventListener("click", () => switchView("list"));
tabCreate.addEventListener("click", () => switchView("create"));

const displaySizeLifecycle = initializeDisplaySizeControl({
  loadInitial: async () => {
    const preferences = await loadPreferences();
    if (!isProbePreferences(preferences)) {
      throw new TypeError("桌面宠物返回了无效的显示偏好");
    }
    return preferences.displayScale;
  },
  createControl: (initial) => new DisplaySizeControl({
    initial,
    request: requestPetDisplayScale,
    elements: {
      slider: displaySizeSlider,
      output: displaySizeOutput,
      status: displaySizeStatus,
      error: displaySizeError,
      presets: displaySizePresets,
    },
  }),
  onError: (error) => {
    displaySizeSlider.disabled = true;
    for (const preset of displaySizePresets) preset.disabled = true;
    displaySizeOutput.textContent = "暂不可用";
    displaySizeError.textContent = "暂时无法读取当前大小。请确认桌面宠物正在运行后重新打开设置。";
  },
});

let calibrationNavigation: { destroy(): void } | undefined;
const destroySettingsPage = (): void => {
  calibrationNavigation?.destroy();
  calibrationNavigation = undefined;
  displaySizeLifecycle.destroy();
  calibrationControl.finalizeClose();
};

const focusCalibrationSection = (): void => {
  switchView("list");
  calibrationSection.scrollIntoView({ block: "start" });
  $<HTMLElement>("calibration-title").focus({ preventScroll: true });
};
calibrationNavigation = await initializeSettingsNavigation({
  listen: async (handler) => listen<unknown>("settings:navigate", ({ payload }) => handler(payload)),
  takePending: () => invoke<string | null>("settings_take_pending_navigation"),
  focusCalibration: focusCalibrationSection,
});
const settingsWindow = getCurrentWindow();
const closeCoordinator = new SettingsCloseCoordinator({
  onCloseRequested: (handler) => settingsWindow.onCloseRequested(handler),
  destroy: () => settingsWindow.destroy(),
  freeze: () => calibrationControl.freezeForClose(),
  unfreeze: () => calibrationControl.unfreezeAfterCloseFailure(),
  settle: () => calibrationControl.settleForClose(),
  restore: () => calibrationControl.restoreBeforeClose(),
  hasActive: () => calibrationControl.needsRestoreBeforeClose(),
  cleanup: destroySettingsPage,
  diagnose: (error) => {
    console.error("[settings] calibration close coordination", error);
  },
});
await closeCoordinator.mount();
wireSettingsPageLifecycle(window, {
  suspend: () => calibrationCatalog.unavailable("设置页面已暂停，校准预览已恢复。"),
  resume: () => { void renderList(); },
  destroy: () => closeCoordinator.beforeUnload(),
});

await displaySizeLifecycle.ready;
await renderList();
if (window.location.hash === "#calibration") focusCalibrationSection();
