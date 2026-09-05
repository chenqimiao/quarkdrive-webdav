// ---------------------------------------------------------------------------
// 滑动窗口预取。
//
// ## 为什么需要它
//
// 夸克对**单条连接的持续吞吐**限速，不是对账号带宽限速。同一个 20 GB 文件、
// 同一条直链、同一时刻实测：
//
//   单流（不管块多大、URL 缓存多久）          ~10 Mbps
//   4 并发拉连续区间，持续 60 秒              303 Mbps
//
// 差 30 倍。这是标准的反下载工具设计：短请求满速，长连接掐死。而原来的
// `read_bytes` 是「一次读 = 一次上游请求」的串行模型，正好落在被惩罚的那一档。
//
// 调参救不了：`--cache-ttl 3600 -S 33554432` 之后 20 秒窗口能跑到 67 Mbps，
// **但 60 秒窗口回落到 8.9**——前 20 秒是爆发假象。所以必须改访问模式，不是改参数。
//
// ## 形状
//
// 维持 `AHEAD` 个 in-flight 的分块请求，`read_bytes` 从最前面那块里切片返回，
// 消费掉一块就补一块。播放器顺序读时窗口一直往前滚；seek 时窗口整个丢掉重建。
//
// 两个实测出来的常数，别随手调：
//
//   CHUNK = 16 MB   再小则请求数上去、每次 TTFB 的占比变大
//   AHEAD = 4       实测 8 并发反而掉到 62 Mbps（对端开始限流），4 是拐点
//
// ## 一个刻意的简化
//
// 窗口只往前滚，不做「乱序读也能命中」的缓存。因为这一层的读者是**播放器**：
// 它要么顺序读，要么跳一次然后继续顺序读。为随机访问做 LRU 只会让状态机复杂
// 一倍，换来一个不存在的场景。
// ---------------------------------------------------------------------------

use std::collections::VecDeque;

use bytes::Bytes;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::drive::QuarkDrive;

/// 一块多大。见文件头：16 MB 是实测的拐点。
pub const CHUNK: u64 = 16 * 1024 * 1024;
/// 同时在途几块。**不是越大越好**，8 并发实测反而更慢。
pub const AHEAD: usize = 4;

/// 一块的取回任务：块号 + 它的 JoinHandle。
struct Pending {
    index: u64,
    task: JoinHandle<Option<Bytes>>,
}

pub struct Prefetcher {
    drive: QuarkDrive,
    url: String,
    /// 文件总长。最后一块要按它截断，否则会向上游要超出文件末尾的 range。
    size: u64,
    /// 在途的块，按块号升序。
    inflight: VecDeque<Pending>,
    /// 下一个要排进窗口的块号。
    next_index: u64,
    /// 当前已就绪的那一块（块号 + 内容）。
    ready: Option<(u64, Bytes)>,
}

impl Prefetcher {
    pub fn new(drive: QuarkDrive, url: String, size: u64, pos: u64) -> Self {
        let mut p = Self {
            drive,
            url,
            size,
            inflight: VecDeque::new(),
            next_index: pos / CHUNK,
            ready: None,
        };
        p.fill();
        p
    }

    /// 这个预取器还能不能服务 `pos`——URL 没换、且 `pos` 落在窗口能到达的范围里。
    ///
    /// 「能到达」的定义很宽松：只要 pos 所在的块号 ≥ 当前就绪块的块号，且不超过
    /// 窗口末端。往回读一律判为不匹配——那是 seek，重建比复用便宜。
    pub fn serves(&self, url: &str, pos: u64) -> bool {
        if self.url != url {
            return false;
        }
        let want = pos / CHUNK;
        let front = self
            .ready
            .as_ref()
            .map(|(i, _)| *i)
            .or_else(|| self.inflight.front().map(|p| p.index));
        match front {
            Some(f) => want >= f && want < self.next_index + AHEAD as u64,
            None => false,
        }
    }

    /// 把窗口补满到 `AHEAD` 块。
    fn fill(&mut self) {
        while self.inflight.len() < AHEAD {
            let index = self.next_index;
            let start = index * CHUNK;
            if start >= self.size {
                break; // 文件末尾，不再排
            }
            // 最后一块按文件长度截断：向上游要超出末尾的 range 会拿到 416。
            let len = std::cmp::min(CHUNK, self.size - start) as usize;
            let drive = self.drive.clone();
            let url = self.url.clone();
            let task = tokio::spawn(async move {
                match drive.download(url, Some((start, len))).await {
                    Ok(b) => Some(b),
                    Err(e) => {
                        warn!(start = start, len = len, error = %e, "prefetch chunk failed");
                        None
                    }
                }
            });
            self.inflight.push_back(Pending { index, task });
            self.next_index += 1;
        }
    }

    /// 取 `pos` 处最多 `count` 字节。
    ///
    /// 返回的可能**少于** `count`——最多给到当前块的末尾。dav-server 会继续循环
    /// 要下一段，所以短读是合法的，而且正是我们想要的：它让「一次读」永远不跨块，
    /// 窗口的推进就变成了一件确定的事。
    pub async fn read(&mut self, pos: u64, count: usize) -> Option<Bytes> {
        let want = pos / CHUNK;

        // 丢掉所有落在目标块之前的块（顺序播放时这里每次丢一块）。
        loop {
            match &self.ready {
                Some((i, _)) if *i == want => break,
                _ => {
                    self.ready = None;
                    match self.inflight.pop_front() {
                        Some(p) => {
                            let data = p.task.await.ok().flatten()?;
                            if p.index == want {
                                self.ready = Some((p.index, data));
                                self.fill();
                                break;
                            }
                            // 不是要的那块，丢掉继续。窗口空了就补。
                            self.fill();
                        }
                        None => {
                            debug!(pos = pos, "prefetch window exhausted");
                            return None;
                        }
                    }
                }
            }
        }

        let (index, data) = self.ready.as_ref()?;
        let offset = (pos - index * CHUNK) as usize;
        if offset >= data.len() {
            return None;
        }
        let end = std::cmp::min(offset + count, data.len());
        Some(data.slice(offset..end))
    }
}

impl Drop for Prefetcher {
    /// seek 或换文件时窗口被丢弃——**必须显式取消在途任务**。
    ///
    /// 不取消的话，那几个请求会继续跑到完成才释放连接。播放器连续拖几次进度条，
    /// 后台就积着十几条对夸克的下载连接——而夸克恰恰是按连接数和时长限速的，
    /// 于是「拖动几次之后就变慢」，一个极难反推的症状。
    fn drop(&mut self) {
        for p in &self.inflight {
            p.task.abort();
        }
    }
}
