//! Holes in VS Code's view. The Code tab's VS Code is a native view above the page, so nothing the
//! page draws (a tooltip, a toast) can rise over it. Where the page draws something over VS Code's
//! area, its layer gets a mask with a hole there, and the page shows through. Only the drawing is cut:
//! clicks in a hole still reach VS Code, so this is for things that are looked at.

/// Masks `wv` (of `size`, logical pixels) with `holes` cut out: rects (x, y, w, h, corner radius)
/// relative to the view's top-left. No holes takes the mask off.
#[cfg(target_os = "macos")]
pub fn cut(wv: &tauri::Webview, size: (f64, f64), holes: Vec<[f64; 5]>) {
    use objc2::{msg_send, rc::Retained, runtime::AnyObject};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::CGMutablePath;
    use objc2_quartz_core::{kCAFillRuleEvenOdd, CALayer, CAShapeLayer};

    let _ = wv.with_webview(move |pw| unsafe {
        let view = pw.inner() as *mut AnyObject;
        let Some(view) = view.as_ref() else { return };
        if holes.is_empty() {
            let layer: Option<Retained<CALayer>> = msg_send![view, layer];
            if let Some(layer) = layer {
                layer.setMask(None);
            }
            return;
        }
        let _: () = msg_send![view, setWantsLayer: true];
        cut_layer(view, size, holes);
    });

    unsafe fn cut_layer(view: &AnyObject, size: (f64, f64), holes: Vec<[f64; 5]>) {
        let layer: Option<Retained<CALayer>> = unsafe { msg_send![view, layer] };
        let Some(layer) = layer else { return };
        let flipped: bool = unsafe { msg_send![view, isFlipped] };
        let (w, h) = size;
        let path = CGMutablePath::new();
        let rect = |x: f64, y: f64, w: f64, h: f64| CGRect::new(CGPoint::new(x, y), CGSize::new(w, h));
        unsafe {
            CGMutablePath::add_rect(Some(&path), std::ptr::null(), rect(0.0, 0.0, w, h));
            for [x, y, hw, hh, r] in holes {
                let y = if flipped { y } else { h - y - hh };
                let r = r.min(hw / 2.0).min(hh / 2.0).max(0.0);
                CGMutablePath::add_rounded_rect(Some(&path), std::ptr::null(), rect(x, y, hw, hh), r, r);
            }
        }
        let mask = CAShapeLayer::new();
        mask.setFrame(rect(0.0, 0.0, w, h));
        mask.setPath(Some(&path));
        mask.setFillRule(unsafe { kCAFillRuleEvenOdd });
        unsafe { layer.setMask(Some(&mask)) };
    }
}

#[cfg(not(target_os = "macos"))]
pub fn cut(_wv: &tauri::Webview, _size: (f64, f64), _holes: Vec<[f64; 5]>) {}
