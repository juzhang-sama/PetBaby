import type { PetCatalogEntry, PetLifecycle } from "../pets/pet-catalog-contract";

export type PetListAction = {
  kind: "edit" | "switch" | "continue" | "delete";
  label: string;
};

export type PetListRow = {
  petId: string;
  title: string;
  detail: string;
  badge?: string;
  actions: PetListAction[];
};

const lifecycleCopy: Record<PetLifecycle, string> = {
  ready: "已准备好，可以出现在桌面上",
  generating: "正在生成形象，可稍后继续",
  generationFailed: "生成未完成，请继续创建后重试",
  awaitingConfirm: "等待确认候选形象",
  compileRetryable: "资源编译未完成，可继续创建",
  awaitingActivation: "等待设为当前宠物",
  corrupt: "本地资料不完整，建议删除后重新创建",
};

const creationMethodLabel: Record<PetCatalogEntry["creationMethod"], string> = {
  upload: "上传创建",
  composer: "引导组合",
  adoption: "直接认领",
};

export function buildPetListRows(entries: PetCatalogEntry[]): PetListRow[] {
  return [...entries]
    .sort((left, right) => Number(right.source === "builtin") - Number(left.source === "builtin"))
    .map((entry) => ({
      petId: entry.petId,
      title: entry.displayName,
      detail: detailFor(entry),
      badge: entry.isCurrent ? "当前使用" : undefined,
      actions: actionsFor(entry),
    }));
}

function actionsFor(entry: PetCatalogEntry): PetListAction[] {
  const actions: PetListAction[] = [];
  if (entry.source === "user") actions.push({ kind: "edit", label: "编辑" });
  if (!entry.isCurrent) {
    if (entry.status === "ready") actions.push({ kind: "switch", label: "设为当前" });
  }
  if (entry.deletable) actions.push({ kind: "delete", label: "删除" });
  return actions;
}

function detailFor(entry: PetCatalogEntry): string {
  if (entry.source === "builtin") return entry.issue ?? lifecycleCopy[entry.status];
  const source = creationMethodLabel[entry.creationMethod];
  return entry.issue ? `${source} · ${entry.issue}` : source;
}
