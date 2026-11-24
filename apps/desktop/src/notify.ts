// A notification from the system (macOS), asking for the permission the first time. A title given
// as a function is worked out once the permission is there, from what is known by then.
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";

export async function systemNotify(title: string | (() => string), body: string) {
  try {
    let ok = await isPermissionGranted();
    if (!ok) ok = (await requestPermission()) === "granted";
    if (ok) sendNotification({ title: typeof title === "function" ? title() : title, body });
  } catch {
    /* the browser preview has no notifications */
  }
}
