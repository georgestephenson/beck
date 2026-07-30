//! Patch frames and their encodings.
//!
//! §4.4 specifies a compact, field-tagged binary encoding for Beck↔Beck traffic. The thin client
//! is a browser: JSON costs it *zero bytes of decoder*, and the decoder is the scarce resource in a
//! 10 KB budget. Phase 0 therefore ships JSON on the wire and keeps the binary encoding alongside
//! it so the trade is a measured number rather than an opinion — `beck-p0-bench payload` reports
//! both, and Phase 1 can move the client to binary knowing exactly what it buys.
//!
//! Every frame carries the `seq` it brings the subscriber up to. That single field is what makes
//! `(subscription, seq)` resumption and, later, optimistic reconciliation cheap (§4.4, §3.7).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::diff::Op;
use crate::log::Seq;
use beck_core::html::Html;

/// A subscription id — content-independent, minted by the client, stable across reconnects.
pub type SubId = String;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchFrame {
    pub seq: Seq,
    pub ops: Vec<Op>,
}

impl PatchFrame {
    pub fn new(seq: Seq, ops: Vec<Op>) -> Self {
        Self { seq, ops }
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// The encoding the thin client consumes: `{"t":"p","q":<seq>,"o":[<op>...]}`.
    pub fn to_json(&self) -> Value {
        json!({
            "t": "p",
            "q": self.seq,
            "o": self.ops.iter().map(Op::to_wire).collect::<Vec<_>>(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// What the thin client speaks.
    Json,
    /// §4.4's field-tagged binary encoding, measured but not yet shipped to the browser.
    Postcard,
}

impl Codec {
    pub fn encode(self, frame: &PatchFrame) -> Vec<u8> {
        match self {
            Codec::Json => serde_json::to_vec(&frame.to_json()).expect("frame is serialisable"),
            Codec::Postcard => {
                postcard::to_allocvec(&WireFrame::from(frame)).expect("frame is serialisable")
            }
        }
    }
}

/// The binary mirror of a frame: same information, no structural hashes, tags instead of names.
#[derive(Serialize, Deserialize)]
struct WireFrame {
    seq: Seq,
    ops: Vec<WireOp>,
}

#[derive(Serialize, Deserialize)]
enum WireOp {
    Replace(Vec<u32>, WireHtml),
    SetText(Vec<u32>, String),
    SetAttr(Vec<u32>, String, String),
    RemoveAttr(Vec<u32>, String),
    Insert(Vec<u32>, u32, WireHtml),
    Remove(Vec<u32>, u32),
    Move(Vec<u32>, u32, u32),
}

#[derive(Serialize, Deserialize)]
enum WireHtml {
    Text(String),
    El {
        tag: String,
        attrs: Vec<(String, String)>,
        key: Option<String>,
        children: Vec<WireHtml>,
    },
}

impl From<&PatchFrame> for WireFrame {
    fn from(frame: &PatchFrame) -> Self {
        WireFrame {
            seq: frame.seq,
            ops: frame.ops.iter().map(WireOp::from).collect(),
        }
    }
}

impl From<&Op> for WireOp {
    fn from(op: &Op) -> Self {
        match op {
            Op::Replace { path, html } => WireOp::Replace(path.clone(), html.into()),
            Op::SetText { path, text } => WireOp::SetText(path.clone(), text.clone()),
            Op::SetAttr { path, name, value } => {
                WireOp::SetAttr(path.clone(), name.clone(), value.clone())
            }
            Op::RemoveAttr { path, name } => WireOp::RemoveAttr(path.clone(), name.clone()),
            Op::Insert { path, index, html } => WireOp::Insert(path.clone(), *index, html.into()),
            Op::Remove { path, index } => WireOp::Remove(path.clone(), *index),
            Op::Move { path, from, to } => WireOp::Move(path.clone(), *from, *to),
        }
    }
}

impl From<&Html> for WireHtml {
    fn from(html: &Html) -> Self {
        match html {
            Html::Text { text, .. } => WireHtml::Text(text.clone()),
            Html::Element {
                tag,
                attrs,
                key,
                children,
                ..
            } => WireHtml::El {
                tag: tag.clone(),
                attrs: attrs.clone(),
                key: key.clone(),
                children: children.iter().map(WireHtml::from).collect(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::diff;

    /// A rendered list of `n` rows, as the compiled `view` produces one.
    fn list(n: usize, done: Option<usize>) -> Html {
        Html::el("ul").children((0..n).map(|i| {
            let row = Html::el("li").key(i.to_string());
            let row = if done == Some(i) {
                row.attr("class", "done")
            } else {
                row
            };
            row.child(Html::text(format!("todo {i}")))
        }))
    }

    #[test]
    fn a_single_toggle_is_a_small_frame_in_both_encodings() {
        let ops = diff(&list(50, None), &list(50, Some(7)));
        let frame = PatchFrame::new(51, ops);
        let json = Codec::Json.encode(&frame);
        let binary = Codec::Postcard.encode(&frame);

        // The point of a patch stream: a 50-row list costs bytes proportional to the change, not
        // to the list. This is the property the whole Mode A design rests on.
        assert!(json.len() < 100, "json frame was {} bytes", json.len());
        assert!(binary.len() < json.len());
    }

    #[test]
    fn the_patch_is_essentially_constant_in_the_size_of_the_list() {
        let small = Codec::Json.encode(&PatchFrame::new(
            1,
            diff(&list(10, None), &list(10, Some(3))),
        ));
        let large = Codec::Json.encode(&PatchFrame::new(
            1,
            diff(&list(1000, None), &list(1000, Some(3))),
        ));
        assert!(
            large.len() <= small.len() + 8,
            "small {} vs large {}",
            small.len(),
            large.len()
        );
    }
}
