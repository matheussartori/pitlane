// A card's images: dropped, picked or pasted (Command V), shown as thumbnails, sent to Claude
// with the spec. Big screenshots are scaled down here first: Claude reads at most ~1568 px.
import { For, Show, createResource, createSignal } from "solid-js";
import { api, type Card } from "./api";
import { Icon } from "./icons";
import { Spinner, toast } from "./ui";
import { ImageViewer } from "./viewer";
import { t } from "./i18n";
import { msg } from "./format";

const EDGE = 1568;
const TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/** Image files in a drop or a paste. */
export function imagesIn(data: DataTransfer | null): File[] {
  if (!data) return [];
  return Array.from(data.files).filter((f) => TYPES.includes(f.type));
}

// Through a data URL: the app's CSP allows data: images, not blob: ones.
async function load(file: Blob): Promise<HTMLImageElement> {
  const url = `data:${file.type};base64,${await toBase64(file)}`;
  return new Promise((ok, fail) => {
    const img = new Image();
    img.onload = () => ok(img);
    img.onerror = () => fail(new Error("not an image the app can read"));
    img.src = url;
  });
}

function toBase64(blob: Blob): Promise<string> {
  return new Promise((ok, fail) => {
    const r = new FileReader();
    r.onload = () => ok(String(r.result).split(",", 2)[1] ?? "");
    r.onerror = () => fail(r.error);
    r.readAsDataURL(blob);
  });
}

/** The file as it goes to the card: scaled to EDGE on its long side, PNG unless that is too big. */
async function prepare(file: File): Promise<{ name: string; data: string }> {
  const base = (file.name || "screenshot").replace(/\.[^.]+$/, "") || "screenshot";
  if (file.type === "image/gif") return { name: `${base}.gif`, data: await toBase64(file) };
  const img = await load(file);
  const scale = Math.min(1, EDGE / Math.max(img.naturalWidth, img.naturalHeight));
  if (scale === 1 && file.size < 3_500_000) {
    const ext = file.type.split("/")[1]!.replace("jpeg", "jpg");
    return { name: `${base}.${ext}`, data: await toBase64(file) };
  }
  const canvas = document.createElement("canvas");
  canvas.width = Math.round(img.naturalWidth * scale);
  canvas.height = Math.round(img.naturalHeight * scale);
  canvas.getContext("2d")!.drawImage(img, 0, 0, canvas.width, canvas.height);
  const blob = (type: string, q?: number) => new Promise<Blob>((ok, fail) => canvas.toBlob((b) => (b ? ok(b) : fail(new Error("could not encode"))), type, q));
  let out = await blob("image/png");
  let ext = "png";
  if (out.size > 3_500_000) {
    out = await blob("image/jpeg", 0.88);
    ext = "jpg";
  }
  return { name: `${base}.${ext}`, data: await toBase64(out) };
}

/** Attaches files to a card, one after another; returns how many made it. */
export async function attachImages(repo: string, id: number, files: File[]): Promise<number> {
  let n = 0;
  for (const f of files) {
    try {
      const p = await prepare(f);
      await api.cardAttach(repo, id, p.name, p.data);
      n++;
    } catch (e) {
      toast({ tone: "fail", title: t("{name} was not attached", { name: f.name || t("The image") }), body: msg(e) }, 0);
    }
  }
  return n;
}

const shown = new Map<string, Promise<string>>();
function src(repo: string, id: number, file: string) {
  const k = `${repo}\n${id}\n${file}`;
  let p = shown.get(k);
  if (!p) {
    p = api.cardImage(repo, id, file);
    p.catch(() => shown.delete(k));
    shown.set(k, p);
  }
  return p;
}

function Thumb(props: { repo: string; id: number; file: string; busy: boolean; onOpen: (url: string) => void; onRemove: () => void }) {
  // An image that cannot be read shows as its name, never as a render error.
  const [url] = createResource(() => src(props.repo, props.id, props.file).catch(() => null));
  return (
    <li class="img-thumb">
      <button type="button" class="img-open" title={props.file} onClick={() => url() && props.onOpen(url()!)}>
        <Show when={url()} fallback={url.loading ? <Spinner /> : <span class="img-missing">{props.file}</span>}>
          <img src={url() ?? undefined} alt={props.file} />
        </Show>
      </button>
      <button type="button" class="btn btn-ghost btn-icon img-remove" aria-label={t("Remove {name}", { name: props.file })} disabled={props.busy} onClick={props.onRemove}>
        <Icon name="close" small />
      </button>
    </li>
  );
}

/** The card sheet's Images section. Paste is handled by the sheet (it calls `add`). */
export function CardImages(props: { repo: string; card: Card; onChanged: () => void; ref?: (add: (files: File[]) => void) => void }) {
  const [busy, setBusy] = createSignal(false);
  const [over, setOver] = createSignal(false);
  const [big, setBig] = createSignal<{ url: string; file: string } | null>(null);
  let picker: HTMLInputElement | undefined;
  const images = () => props.card.images ?? [];

  async function add(files: File[]) {
    if (!files.length) return;
    setBusy(true);
    try {
      if (await attachImages(props.repo, props.card.id, files)) props.onChanged();
    } finally {
      setBusy(false);
    }
  }
  props.ref?.((f) => void add(f));

  async function remove(file: string) {
    setBusy(true);
    try {
      await api.cardDetach(props.repo, props.card.id, file);
      // Its name is free again: the next image may take it, and must not show this one's picture.
      shown.delete(`${props.repo}\n${props.card.id}\n${file}`);
      props.onChanged();
    } catch (e) {
      toast({ tone: "fail", title: t("The image was not removed"), body: msg(e) }, 0);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section
      class="cs-section img-zone"
      data-over={over() ? "" : undefined}
      onDragOver={(e) => {
        if (!e.dataTransfer?.types.includes("Files")) return;
        e.preventDefault();
        setOver(true);
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(false);
      }}
      onDrop={(e) => {
        e.preventDefault();
        setOver(false);
        void add(imagesIn(e.dataTransfer));
      }}
    >
      <div class="field-head">
        <h3 class="cs-label">
          {t("Images")} <span class="count">{images().length}</span>
        </h3>
        <Show when={busy()}>
          <Spinner />
        </Show>
        <button type="button" class="btn btn-ghost cs-act" disabled={busy()} onClick={() => picker?.click()}>
          <Icon name="plus" small /> {t("Add")}
        </button>
        <input
          ref={picker}
          type="file"
          accept={TYPES.join(",")}
          multiple
          hidden
          onChange={(e) => {
            void add(Array.from(e.currentTarget.files ?? []));
            e.currentTarget.value = "";
          }}
        />
      </div>
      <Show when={images().length > 0} fallback={
          <button type="button" class="img-drop" disabled={busy()} onClick={() => picker?.click()}>
            <Icon name="plus" small />
            {t("Drop screenshots here, pick them with Add, or paste with ⌘V. Claude gets them with the spec.")}
          </button>
        }>
        <ul class="img-list">
          <For each={images()}>
            {(f) => <Thumb repo={props.repo} id={props.card.id} file={f} busy={busy()} onOpen={(url) => setBig({ url, file: f })} onRemove={() => void remove(f)} />}
          </For>
        </ul>
      </Show>
      <ImageViewer open={!!big()} src={big()?.url ?? ""} name={big()?.file} alt="" label={t("Image")} onClose={() => setBig(null)} />
    </section>
  );
}
