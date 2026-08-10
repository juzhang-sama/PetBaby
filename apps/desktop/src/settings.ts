import { invoke } from "@tauri-apps/api/core";
import { creationApi } from "./creation/api";
import type { PetCatalogEntry } from "./pets/pet-catalog-contract";
import { CandidatePreviewController } from "./settings/candidate-dynamic-preview";
import { finalizeCreation } from "./settings/creation-finalizer";
import {
  catalogSwitchStatus,
  deleteCurrentCatalogPet,
  mergeCatalogWarnings,
} from "./settings/pet-catalog-delete-flow";
import { buildPetListRows, type PetListAction } from "./settings/pet-catalog-view-model";
import { requestPetSwitch } from "./settings/pet-switch-client";
import {
  queryUploadCreationElements,
  type CandidateDynamicAssets,
  UploadCreationView,
} from "./settings/upload-creation-view";

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

let catalogEntries: PetCatalogEntry[] = [];
let catalogBusy: "switch" | "delete" | null = null;
let selectedView: "list" | "create" = "list";

function setCatalogStatus(message: string, tone: "info" | "error" | "warning" = "info"): void {
  statusEl.textContent = message;
  statusEl.classList.toggle("error", tone === "error");
  statusEl.classList.toggle("warning", tone === "warning");
}

function renderCatalogRows(): void {
  listEl.replaceChildren();
  if (catalogEntries.length === 0) {
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.textContent = "还没有宠物。前往“创建宠物”上传一张猫咪照片吧。";
    listEl.append(empty);
    return;
  }
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
      button.addEventListener("click", () => { void handleCatalogAction(row.petId, action); });
      actions.append(button);
    }
    item.append(copy, actions);
    listEl.append(item);
  }
}

async function renderList(): Promise<boolean> {
  try {
    catalogEntries = await invoke<PetCatalogEntry[]>("pet_catalog_list");
    renderCatalogRows();
    return true;
  } catch (error) {
    setCatalogStatus(`读取宠物目录失败。请确认桌面宠物正在运行后重试：${String(error)}`, "error");
    return false;
  }
}

async function handleCatalogAction(petId: string, action: PetListAction): Promise<void> {
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

const candidatePreview = new CandidatePreviewController();
const uploadView = new UploadCreationView(
  {
    creation: creationApi,
    finalize: finalizeCreation,
  },
  {
    elements: queryUploadCreationElements(document),
    createElement: (tagName) => document.createElement(tagName),
    loadApiKey: () => invoke<string | null>("app_setting_get", { key: "lk888_api_key" }),
    saveApiKey: (value) => invoke<void>("app_setting_set", { key: "lk888_api_key", value }),
    loadCandidate: (jobId) =>
      invoke<CandidateDynamicAssets>("creation_upload_candidate_assets", { jobId }),
    preview: candidatePreview,
    setInterval: (callback, delayMs) => window.setInterval(callback, delayMs),
    clearInterval: (id) => window.clearInterval(id),
    createObjectURL: (file) => URL.createObjectURL(file),
    revokeObjectURL: (url) => URL.revokeObjectURL(url),
    confirm: (message) => window.confirm(message),
    onCancel: () => switchView("list"),
    onAbandoned: () => {
      setCatalogStatus("已放弃创建并清理本地草稿。");
      switchView("list");
    },
  },
);
uploadView.mount();

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
    uploadView.leave();
    void renderList();
  } else {
    void uploadView.enter();
  }
}

tabList.addEventListener("click", () => switchView("list"));
tabCreate.addEventListener("click", () => switchView("create"));

await renderList();
