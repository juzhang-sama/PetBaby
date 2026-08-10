import type { PetCatalogEntry, PetLifecycle } from "../pets/pet-catalog-contract";

export type PetListAction = {
  kind: "switch" | "continue" | "delete";
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

const speciesLabel: Record<PetCatalogEntry["species"], string> = {
  cat: "猫咪",
  dog: "狗狗",
};

const resumable: ReadonlySet<PetLifecycle> = new Set([
  "generating",
  "generationFailed",
  "awaitingConfirm",
  "compileRetryable",
  "awaitingActivation",
]);

export function buildPetListRows(entries: PetCatalogEntry[]): PetListRow[] {
  return [...entries]
    .sort((left, right) => Number(right.source === "builtin") - Number(left.source === "builtin"))
    .map((entry) => ({
      petId: entry.petId,
      title: entry.source === "builtin"
        ? "默认猫 · Live2D"
        : `${speciesLabel[entry.species]} · ${formatCreatedAt(entry.createdAt)}`,
      detail: entry.issue ?? lifecycleCopy[entry.status],
      badge: entry.isCurrent ? "当前使用" : undefined,
      actions: actionsFor(entry),
    }));
}

function actionsFor(entry: PetCatalogEntry): PetListAction[] {
  const actions: PetListAction[] = [];
  if (!entry.isCurrent) {
    if (entry.status === "ready") actions.push({ kind: "switch", label: "设为当前" });
    if (resumable.has(entry.status)) actions.push({ kind: "continue", label: "继续创建" });
  }
  if (entry.deletable) actions.push({ kind: "delete", label: "删除" });
  return actions;
}

function formatCreatedAt(createdAt: string | null): string {
  if (!createdAt) return "创建日期未知";
  const date = new Date(createdAt);
  if (Number.isNaN(date.getTime())) return "创建日期未知";
  return new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "short", day: "numeric" }).format(date);
}
