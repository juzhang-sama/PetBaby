import { invoke } from "@tauri-apps/api/core";
import { CreationFlow, type CreationStep } from "./creation/creation-flow";
import type { CreationResume, PetCatalogEntry } from "./pets/pet-catalog-contract";
import { deleteCurrentCatalogPet } from "./settings/pet-catalog-delete-flow";
import { buildPetListRows, type PetListAction } from "./settings/pet-catalog-view-model";
import { requestPetSwitch } from "./settings/pet-switch-client";
import {
  CreationWizardRun,
  refreshFailureDisposition,
  resumeDisposition,
  type WizardOperationToken,
} from "./settings/creation-wizard-run";

interface PetSummary { petId: string; }
interface JobInfo { jobId: string; status: string; error: string | null; }
interface DeleteOutcome { warning: string | null; }

const $ = <T extends HTMLElement>(id: string): T => {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el as T;
};

const listEl = $<HTMLDivElement>("list");
const statusEl = $<HTMLDivElement>("status");
const tabList = $<HTMLButtonElement>("tab-list");
const tabCreate = $<HTMLButtonElement>("tab-create");
const viewList = $<HTMLDivElement>("view-list");
const viewCreate = $<HTMLDivElement>("view-create");

let catalogEntries: PetCatalogEntry[] = [];
let catalogBusy: "switch" | "delete" | null = null;

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
    empty.textContent = "还没有宠物。前往“创建宠物”新建一只吧。";
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
    enterCreationResume(petId);
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
    if (await renderList()) setCatalogStatus("已设为当前桌面宠物。");
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
    if (entry.isCurrent) {
      const result = await deleteCurrentCatalogPet({
        switchToBuiltin: async () => {
          const switched = await requestPetSwitch("pet-live2d-v1");
          return switched.ok ? { ok: true } : { ok: false, message: switched.message };
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
    } else {
      outcome = await invoke<DeleteOutcome>("pet_delete_full", { petId });
    }
    if (await renderList()) setCatalogStatus(outcome.warning ?? "宠物已删除。", outcome.warning ? "warning" : "info");
  } catch (error) {
    setCatalogStatus(`删除失败。请稍后重试：${String(error)}`, "error");
  } finally {
    catalogBusy = null;
    renderCatalogRows();
  }
}

// --- resumable, single-candidate creation wizard ---
const wSpecies = $<HTMLSelectElement>("w-species");
const photoInput = $<HTMLInputElement>("photo-input");
const photoPreview = $<HTMLImageElement>("photo-preview");
const apiKeyInput = $<HTMLInputElement>("api-key");
const saveKeyBtn = $<HTMLButtonElement>("save-key");
const keyStatus = $<HTMLDivElement>("key-status");
const wizardStatus = $<HTMLDivElement>("wizard-status");
const jobGrid = $<HTMLDivElement>("job-grid");
const candidateGrid = $<HTMLDivElement>("candidate-grid");
const stepUpload = $<HTMLDivElement>("step-upload");
const stepGenerating = $<HTMLDivElement>("step-generating");
const stepReview = $<HTMLDivElement>("step-review");
const stepConfirm = $<HTMLDivElement>("step-confirm");
const stepComplete = $<HTMLDivElement>("step-complete");
const btnNext = $<HTMLButtonElement>("wizard-next");
const btnCancel = $<HTMLButtonElement>("wizard-cancel");
const reviewActions = $<HTMLDivElement>("review-actions");
const reviewAccept = $<HTMLButtonElement>("review-accept");
const reviewRetry = $<HTMLButtonElement>("review-retry");
const reviewAbandon = $<HTMLButtonElement>("review-abandon");
const confirmRetry = $<HTMLButtonElement>("confirm-retry");

const creationResumeStorageKey = "desktop-pet.creation.resumePetId";
let flow = new CreationFlow();
let photoBytes: Uint8Array | null = null;
let pollTimer: number | undefined;
let generationRefreshVisit: number | null = null;
let wizardVisit = 0;
const wizardRun = new CreationWizardRun();

function persistCreationPet(petId: string): void {
  window.sessionStorage.setItem(creationResumeStorageKey, petId);
}

function clearCreationPet(petId?: string): void {
  if (!petId || window.sessionStorage.getItem(creationResumeStorageKey) === petId) {
    window.sessionStorage.removeItem(creationResumeStorageKey);
  }
}

function stopPolling(): void {
  if (pollTimer !== undefined) window.clearInterval(pollTimer);
  pollTimer = undefined;
}

function resetPhoto(): void {
  photoBytes = null;
  flow.clearPhoto();
  photoInput.value = "";
  photoPreview.removeAttribute("src");
  photoPreview.style.display = "none";
}

function syncWizardOperationControls(petId: string | null): void {
  const busy = wizardRun.isPetBusy(petId);
  reviewAccept.disabled = busy;
  reviewRetry.disabled = busy;
  reviewAbandon.disabled = busy;
  confirmRetry.disabled = busy;
}

function isCurrentWizard(visit: number, expectedFlow: CreationFlow): boolean {
  return wizardRun.isCurrent(visit) && flow === expectedFlow;
}

function persistCreationPetForVisit(petId: string, visit: number, expectedFlow: CreationFlow): void {
  if (wizardRun.shouldPersistPet(visit) && isCurrentWizard(visit, expectedFlow)) {
    persistCreationPet(petId);
  }
}

function showStep(step: CreationStep): void {
  stepUpload.style.display = step === "upload" ? "" : "none";
  stepGenerating.style.display = step === "generating" ? "" : "none";
  stepReview.style.display = step === "review" ? "" : "none";
  stepConfirm.style.display = step === "confirm" ? "" : "none";
  stepComplete.style.display = step === "complete" ? "" : "none";
  btnNext.style.display = step === "upload" ? "" : "none";
  btnCancel.style.display = step === "generating" ? "" : "none";
}

async function loadApiKey(): Promise<void> {
  try {
    const key = await invoke<string | null>("app_setting_get", { key: "lk888_api_key" });
    if (key) apiKeyInput.value = key;
  } catch { /* optional convenience only */ }
}

saveKeyBtn.addEventListener("click", async () => {
  const key = apiKeyInput.value.trim();
  if (!key) {
    keyStatus.textContent = "请输入 API Key";
    return;
  }
  try {
    await invoke("app_setting_set", { key: "lk888_api_key", value: key });
    keyStatus.textContent = "已保存";
  } catch (error) {
    keyStatus.textContent = `保存失败: ${String(error)}`;
  }
});

function startWizard(): void {
  stopPolling();
  wizardVisit = wizardRun.enter();
  void loadApiKey();
  flow = new CreationFlow();
  flow.setSpecies(wSpecies.value as "cat" | "dog");
  resetPhoto();
  wizardStatus.textContent = "";
  jobGrid.replaceChildren();
  candidateGrid.replaceChildren();
  reviewActions.style.display = "none";
  syncWizardOperationControls(null);
  btnNext.disabled = false;
  showStep("upload");
}

function enterCreationResume(petId: string): void {
  persistCreationPet(petId);
  switchView("create");
}

async function restoreWizardView(): Promise<void> {
  wizardVisit = wizardRun.enter();
  const visit = wizardVisit;
  const petId = window.sessionStorage.getItem(creationResumeStorageKey);
  if (!petId) {
    startWizard();
    return;
  }
  stopPolling();
  wizardStatus.textContent = "正在恢复创建进度…";
  try {
    const restoredFlow = new CreationFlow();
    flow = restoredFlow;
    syncWizardOperationControls(petId);
    const snapshot = await invoke<CreationResume>("pet_creation_resume", { petId });
    if (!isCurrentWizard(visit, restoredFlow)) return;
    if (resumeDisposition(snapshot.status, true) === "corrupt") {
      handleCorruptSnapshot(snapshot, visit, restoredFlow);
      return;
    }
    restoredFlow.restore(snapshot);
    persistCreationPet(snapshot.petId);
    resetPhoto();
    await renderResumedSnapshot(snapshot, visit, restoredFlow);
  } catch (error) {
    if (!isCurrentWizard(visit, flow)) return;
    wizardStatus.textContent = `无法恢复创建进度：${String(error)}`;
    showStep("upload");
  }
}

async function renderResumedSnapshot(snapshot: CreationResume, visit: number, expectedFlow: CreationFlow): Promise<void> {
  if (!isCurrentWizard(visit, expectedFlow)) return;
  syncWizardOperationControls(expectedFlow.petId);
  showStep(expectedFlow.step);
  if (expectedFlow.step === "generating") {
    await refreshGenerating(visit, expectedFlow);
    if (isCurrentWizard(visit, expectedFlow) && expectedFlow.step === "generating") startPolling(visit, expectedFlow);
    return;
  }
  if (expectedFlow.step === "upload") {
    wizardStatus.textContent = `上次生成失败：${snapshot.error ?? "未知原因"}。请重新选择照片后重试。`;
    return;
  }
  if (expectedFlow.step === "review") {
    await renderCandidate(snapshot.jobId, visit, expectedFlow);
    return;
  }
  if (expectedFlow.step === "confirm") {
    wizardStatus.textContent = "资产已准备，正在设为当前宠物…";
    void activatePreparedCandidate(visit, expectedFlow);
    return;
  }
  wizardStatus.textContent = "宠物已出现在桌面。";
  clearCreationPet(snapshot.petId);
}

photoInput.addEventListener("change", async () => {
  const file = photoInput.files?.[0];
  if (!file) return;
  photoBytes = new Uint8Array(await file.arrayBuffer());
  photoPreview.src = URL.createObjectURL(file);
  photoPreview.style.display = "block";
});

async function saveApiKeyForGeneration(visit: number, expectedFlow: CreationFlow): Promise<boolean> {
  const key = apiKeyInput.value.trim();
  if (!key) {
    if (isCurrentWizard(visit, expectedFlow)) wizardStatus.textContent = "请先在上方填写并保存 API Key";
    return false;
  }
  try {
    await invoke("app_setting_set", { key: "lk888_api_key", value: key });
    if (!isCurrentWizard(visit, expectedFlow)) return false;
    keyStatus.textContent = "Key 已自动保存";
    return true;
  } catch (error) {
    if (isCurrentWizard(visit, expectedFlow)) wizardStatus.textContent = `保存 Key 失败: ${String(error)}`;
    return false;
  }
}

async function ensurePetForSubmission(targetFlow: CreationFlow, visit: number): Promise<void> {
  if (targetFlow.petId) return;
  const pet = await invoke<PetSummary>("pet_create", {
    species: wSpecies.value,
    identityMode: "realPet",
  });
  if (wizardRun.shouldCompensateCreatedPet(visit) || !isCurrentWizard(visit, targetFlow)) {
    try {
      await invoke<DeleteOutcome>("pet_delete_full", { petId: pet.petId });
    } catch {
      // A stale creation must never overwrite the newer wizard's UI or resume key.
    }
    return;
  }
  targetFlow.setPetId(pet.petId);
  persistCreationPetForVisit(pet.petId, visit, targetFlow);
}

btnNext.addEventListener("click", async () => {
  if (!photoBytes) {
    wizardStatus.textContent = "请先选择宠物照片";
    return;
  }
  const visit = wizardVisit;
  const submittingFlow = flow;
  if (!wizardRun.beginSubmission(visit)) return;
  btnNext.disabled = true;
  try {
    if (!await saveApiKeyForGeneration(visit, submittingFlow) || !isCurrentWizard(visit, submittingFlow)) return;
    wizardStatus.textContent = "准备生成…";
    submittingFlow.setSpecies(wSpecies.value as "cat" | "dog");
    await ensurePetForSubmission(submittingFlow, visit);
    if (!isCurrentWizard(visit, submittingFlow)) return;
    submittingFlow.setPhotoBytes(photoBytes);
    if (!submittingFlow.petId || wizardRun.beginGeneration(visit, submittingFlow.petId) === null) return;
    await submittingFlow.submitSingle();
    persistCreationPetForVisit(submittingFlow.petId!, visit, submittingFlow);
    if (!isCurrentWizard(visit, submittingFlow)) return;
    wizardStatus.textContent = "";
    showStep("generating");
    await refreshGenerating(visit, submittingFlow);
    if (isCurrentWizard(visit, submittingFlow) && submittingFlow.step === "generating") startPolling(visit, submittingFlow);
  } catch (error) {
    if (isCurrentWizard(visit, submittingFlow)) wizardStatus.textContent = `提交生成任务失败: ${String(error)}`;
  } finally {
    wizardRun.endSubmission(visit);
    if (isCurrentWizard(visit, submittingFlow)) btnNext.disabled = false;
  }
});

function startPolling(visit: number, expectedFlow: CreationFlow): void {
  stopPolling();
  pollTimer = window.setInterval(() => { void refreshGenerating(visit, expectedFlow); }, 4000);
}

async function refreshGenerating(visit: number, expectedFlow: CreationFlow): Promise<void> {
  if (!isCurrentWizard(visit, expectedFlow) || expectedFlow.step !== "generating" || !expectedFlow.petId) return;
  if (generationRefreshVisit === visit) return;
  generationRefreshVisit = visit;
  const petId = expectedFlow.petId;
  try {
    const [jobs, snapshot] = await Promise.all([
      invoke<JobInfo[]>("gen_list", { petId }),
      expectedFlow.poll(),
    ]);
    if (!isCurrentWizard(visit, expectedFlow) || expectedFlow.petId !== petId) return;
    const job = jobs.find((item) => item.jobId === snapshot.jobId);
    renderJob(job, snapshot);
    if (expectedFlow.step === "generating") return;
    stopPolling();
    await renderResumedSnapshot(snapshot, visit, expectedFlow);
  } catch (error) {
    if (isCurrentWizard(visit, expectedFlow)) wizardStatus.textContent = `查询生成进度失败: ${String(error)}`;
  } finally {
    if (generationRefreshVisit === visit) generationRefreshVisit = null;
  }
}

function renderJob(job: JobInfo | undefined, snapshot: CreationResume): void {
  jobGrid.replaceChildren();
  const card = document.createElement("div");
  const status = job?.status ?? snapshot.status;
  card.className = `job-card ${status === "success" ? "success" : status === "failed" ? "failed" : ""}`;
  const labels: Record<string, string> = {
    pending: "排队中", running: "生成中…", success: "生成完成", failed: "生成失败", generationFailed: "生成失败",
  };
  card.textContent = `${labels[status] ?? status}${job?.error || snapshot.error ? `（${job?.error ?? snapshot.error}）` : ""}`;
  jobGrid.append(card);
}

async function renderCandidate(jobId: string | null, visit: number, expectedFlow: CreationFlow): Promise<void> {
  if (!isCurrentWizard(visit, expectedFlow)) return;
  syncWizardOperationControls(expectedFlow.petId);
  candidateGrid.replaceChildren();
  reviewActions.style.display = "";
  if (!jobId) {
    wizardStatus.textContent = "候选记录不完整。请重新选择照片后重试。";
    return;
  }
  try {
    const card = document.createElement("div");
    card.className = "candidate selected";
    const image = document.createElement("img");
    image.src = await invoke<string>("gen_cutout_b64", { jobId });
    if (!isCurrentWizard(visit, expectedFlow)) return;
    image.alt = "生成的宠物候选";
    const label = document.createElement("div");
    label.className = "candidate-id";
    label.textContent = jobId;
    card.append(image, label);
    candidateGrid.append(card);
    wizardStatus.textContent = "";
  } catch (error) {
    if (isCurrentWizard(visit, expectedFlow)) wizardStatus.textContent = `候选图片暂不可用：${String(error)}`;
  }
}

async function refreshCurrentVisitForPet(operation: WizardOperationToken): Promise<void> {
  const visit = wizardVisit;
  const currentFlow = flow;
  const refresh = wizardRun.beginRefresh(visit, operation.petId, operation.revision);
  if (!refresh || !isCurrentWizard(visit, currentFlow) || currentFlow.petId !== operation.petId) return;
  try {
    const snapshot = await invoke<CreationResume>("pet_creation_resume", { petId: operation.petId });
    if (!isCurrentWizard(visit, currentFlow) || !wizardRun.shouldApplyRefresh(refresh, currentFlow.petId)) {
      recoverRefreshControls(refresh, currentFlow);
      return;
    }
    if (resumeDisposition(snapshot.status, true) === "corrupt") {
      handleCorruptSnapshot(snapshot, visit, currentFlow);
      return;
    }
    currentFlow.restore(snapshot);
    syncWizardOperationControls(currentFlow.petId);
    await renderResumedSnapshot(snapshot, visit, currentFlow);
  } catch {
    recoverRefreshControls(refresh, currentFlow);
  }
}

function handleCorruptSnapshot(snapshot: CreationResume, visit: number, expectedFlow: CreationFlow): void {
  if (!isCurrentWizard(visit, expectedFlow) || (expectedFlow.petId !== null && expectedFlow.petId !== snapshot.petId)) return;
  stopPolling();
  clearCreationPet(snapshot.petId);
  reviewAccept.disabled = true;
  reviewRetry.disabled = true;
  reviewAbandon.disabled = true;
  confirmRetry.disabled = true;
  switchView("list");
  setCatalogStatus("本地资料损坏，请删除后重新创建", "error");
}

function recoverRefreshControls(refresh: { visit: number; petId: string; revision: number }, currentFlow: CreationFlow): void {
  const currentVisitSamePet = isCurrentWizard(refresh.visit, currentFlow) && currentFlow.petId === refresh.petId;
  const disposition = refreshFailureDisposition({
    currentVisitSamePet,
    revisionMatches: wizardRun.shouldApplyRefresh(refresh, currentFlow.petId),
  });
  if (!disposition.syncControls) return;
  syncWizardOperationControls(currentFlow.petId);
  if (disposition.message) wizardStatus.textContent = disposition.message;
}

function refreshAfterStaleOperation(operation: WizardOperationToken): void {
  if (wizardRun.shouldRefreshStaleOperation(operation, flow.petId)) {
    void refreshCurrentVisitForPet(operation);
  }
}

reviewAccept.addEventListener("click", async () => {
  const visit = wizardVisit;
  const reviewFlow = flow;
  const petId = reviewFlow.petId;
  if (!petId || !isCurrentWizard(visit, reviewFlow)) return;
  const operation = wizardRun.beginOperation(visit, "compile", petId);
  if (!operation) return;
  syncWizardOperationControls(petId);
  wizardStatus.textContent = "正在编译资产…";
  let compiled: { degraded: boolean } | null = null;
  try {
    compiled = await reviewFlow.compileCandidate();
    if (!isCurrentWizard(visit, reviewFlow)) return;
    showStep("confirm");
    wizardStatus.textContent = compiled.degraded ? "资产已准备（降级模式），正在设为当前宠物…" : "资产已准备，正在设为当前宠物…";
  } catch (error) {
    if (isCurrentWizard(visit, reviewFlow)) {
      wizardStatus.textContent = `编译失败，可重试：${String(error)}`;
      showStep("review");
    }
  } finally {
    wizardRun.settleOperation(operation);
    if (isCurrentWizard(visit, reviewFlow)) syncWizardOperationControls(petId);
    else refreshAfterStaleOperation(operation);
  }
  if (compiled && isCurrentWizard(visit, reviewFlow)) await activatePreparedCandidate(visit, reviewFlow);
});

async function activatePreparedCandidate(visit: number, expectedFlow: CreationFlow): Promise<void> {
  const petId = expectedFlow.petId;
  if (!petId || !isCurrentWizard(visit, expectedFlow) || expectedFlow.step !== "confirm") return;
  const operation = wizardRun.beginOperation(visit, "activate", petId);
  if (!operation) return;
  syncWizardOperationControls(petId);
  try {
    await expectedFlow.activateCandidate();
    clearCreationPet(petId);
    if (!isCurrentWizard(visit, expectedFlow)) return;
    showStep("complete");
    wizardStatus.textContent = "宠物已出现在桌面。";
  } catch (error) {
    if (!isCurrentWizard(visit, expectedFlow)) return;
    showStep("confirm");
    wizardStatus.textContent = `资产已准备，可重试设为当前宠物：${String(error)}`;
  } finally {
    wizardRun.settleOperation(operation);
    if (isCurrentWizard(visit, expectedFlow)) syncWizardOperationControls(petId);
    else refreshAfterStaleOperation(operation);
  }
}

confirmRetry.addEventListener("click", () => { void activatePreparedCandidate(wizardVisit, flow); });

reviewRetry.addEventListener("click", () => {
  if (!isCurrentWizard(wizardVisit, flow) || wizardRun.isPetBusy(flow.petId)) return;
  resetPhoto();
  reviewActions.style.display = "none";
  candidateGrid.replaceChildren();
  showStep("upload");
  wizardStatus.textContent = "请重新选择照片，再重新生成。";
});

reviewAbandon.addEventListener("click", async () => {
  const petId = flow.petId;
  if (!petId || wizardRun.isPetBusy(petId) || !window.confirm("确定放弃创建吗？这会删除这只宠物的本地资料和生成任务。")) return;
  const visit = wizardVisit;
  const reviewFlow = flow;
  if (!isCurrentWizard(visit, reviewFlow)) return;
  reviewAbandon.disabled = true;
  reviewAccept.disabled = true;
  reviewRetry.disabled = true;
  wizardStatus.textContent = "正在删除创建记录…";
  try {
    const outcome = await invoke<DeleteOutcome>("pet_delete_full", { petId });
    if (!isCurrentWizard(visit, reviewFlow)) {
      clearCreationPet(petId);
      return;
    }
    clearCreationPet(petId);
    if (outcome.warning) setCatalogStatus(outcome.warning, "warning");
    switchView("list");
    if (!outcome.warning) setCatalogStatus("已放弃创建并删除本地资料。");
  } catch (error) {
    if (isCurrentWizard(visit, reviewFlow)) {
      wizardStatus.textContent = `删除失败，仍可继续创建或重试删除：${String(error)}`;
      reviewAbandon.disabled = false;
      reviewAccept.disabled = false;
      reviewRetry.disabled = false;
    }
  }
});

btnCancel.addEventListener("click", () => switchView("list"));

function switchView(view: "list" | "create"): void {
  if (view === "list") {
    stopPolling();
    wizardRun.leave();
    wizardVisit = 0;
  }
  viewList.style.display = view === "list" ? "" : "none";
  viewCreate.style.display = view === "create" ? "" : "none";
  tabList.classList.toggle("active", view === "list");
  tabCreate.classList.toggle("active", view === "create");
  if (view === "list") void renderList();
  if (view === "create") void restoreWizardView();
}

tabList.addEventListener("click", () => switchView("list"));
tabCreate.addEventListener("click", () => switchView("create"));

await renderList();
