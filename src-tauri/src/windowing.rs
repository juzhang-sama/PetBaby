use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RegionSpan {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HitRegionPayload {
    pub canvas_width: i32,
    pub canvas_height: i32,
    pub scale_factor: f64,
    pub spans: Vec<RegionSpan>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HitRegionEvidence {
    pub span_count: usize,
    pub applied: bool,
    pub strategy: &'static str,
    pub scale_factor: f64,
}

pub fn normalize_spans(payload: &HitRegionPayload) -> Result<Vec<RegionSpan>, &'static str> {
    if payload.canvas_width <= 0 || payload.canvas_height <= 0 || payload.scale_factor <= 0.0 {
        return Err("canvas dimensions must be positive");
    }
    let spans = payload
        .spans
        .iter()
        .filter_map(|span| {
            let clipped = RegionSpan {
                left: span.left.clamp(0, payload.canvas_width),
                top: span.top.clamp(0, payload.canvas_height),
                right: span.right.clamp(0, payload.canvas_width),
                bottom: span.bottom.clamp(0, payload.canvas_height),
            };
            (clipped.left < clipped.right && clipped.top < clipped.bottom).then_some(clipped)
        })
        .collect();
    Ok(spans)
}

pub fn scale_spans(spans: &[RegionSpan], scale_factor: f64) -> Vec<RegionSpan> {
    spans
        .iter()
        .map(|span| RegionSpan {
            left: (span.left as f64 * scale_factor).floor() as i32,
            top: (span.top as f64 * scale_factor).floor() as i32,
            right: (span.right as f64 * scale_factor).ceil() as i32,
            bottom: (span.bottom as f64 * scale_factor).ceil() as i32,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_positive_canvas() {
        let payload = HitRegionPayload {
            canvas_width: 0,
            canvas_height: 10,
            scale_factor: 1.0,
            spans: vec![],
        };
        assert_eq!(
            normalize_spans(&payload),
            Err("canvas dimensions must be positive")
        );
    }

    #[test]
    fn clips_spans_and_removes_empty_rows() {
        let payload = HitRegionPayload {
            canvas_width: 100,
            canvas_height: 50,
            scale_factor: 1.0,
            spans: vec![
                RegionSpan {
                    left: -5,
                    top: 2,
                    right: 20,
                    bottom: 4,
                },
                RegionSpan {
                    left: 30,
                    top: 3,
                    right: 30,
                    bottom: 5,
                },
            ],
        };
        assert_eq!(
            normalize_spans(&payload).unwrap(),
            vec![RegionSpan {
                left: 0,
                top: 2,
                right: 20,
                bottom: 4
            },]
        );
    }

    #[test]
    fn scales_css_spans_outward_for_high_dpi_windows() {
        let scaled = scale_spans(
            &[RegionSpan {
                left: 1,
                top: 2,
                right: 3,
                bottom: 4,
            }],
            1.5,
        );
        assert_eq!(
            scaled,
            vec![RegionSpan {
                left: 1,
                top: 3,
                right: 5,
                bottom: 6
            }]
        );
    }
}
