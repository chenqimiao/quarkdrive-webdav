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

/// 已拉回的块，按 (fid, 块号) 共享。
///
/// **必须挂在文件系统层，不能挂在文件句柄上。** dav-server 每个 HTTP 请求都会走
/// 一遍 `open()` 造一个新的 DavFile，而播放器（实测 Infuse）每隔十几秒就换一条
/// 连接重发 range 请求。窗口跟着句柄一起死的话，已经拉回来但还没送出的块全部作废
/// ——实测放大比 1.35×，即 26% 的上游流量是白拉的。
///
/// 在家里无所谓（下行富余），但远程时这 26% 是直接乘在家宽上行天花板上的。
pub type ChunkCache = moka::future::Cache<(String, u64), Bytes>;

/// 建一个按**字节数**限容的块缓存。
///
/// 用 weigher 而不是条数：块大小虽然名义上是 CHUNK，但文件最后一块会被截断，
/// 按条数算会低估占用。
pub fn new_chunk_cache(max_bytes: u64) -> ChunkCache {
    moka::future::Cache::builder()
        .max_capacity(max_bytes)
        .weigher(|_k: &(String, u64), v: &Bytes| v.len().try_into().unwrap_or(u32::MAX))
        // 直链本身几十分钟就过期，块留得比它久没有意义。
        .time_to_live(std::time::Duration::from_secs(300))
        .build()
}

/// 全局在途分块下载的上限。
///
/// 窗口被丢弃时**不再 abort 在途任务**（见 `Drop`），所以理论上快速连续 seek 会
/// 攒出很多孤儿任务。这个信号量是那件事的兜底：不管有多少个窗口活着或死了，同时
/// 真正在打夸克的分块请求不超过这个数。
///
/// 8 = AHEAD 的两倍：允许「上一个窗口正在收尾」和「新窗口已经启动」短暂重叠，
/// 但不允许再多——夸克是按连接维度限速的。
static DOWNLOAD_SLOTS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();

fn slots() -> &'static tokio::sync::Semaphore {
    DOWNLOAD_SLOTS.get_or_init(|| tokio::sync::Semaphore::new(AHEAD * 2))
}

/// 一块的取回任务：块号 + 它的 JoinHandle。
struct Pending {
    index: u64,
    task: JoinHandle<Option<Bytes>>,
}

pub struct Prefetcher {
    drive: QuarkDrive,
    url: String,
    /// 文件 id。缓存键的一半——直链会换，fid 不会。
    fid: String,
    cache: ChunkCache,
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
    pub fn new(
        drive: QuarkDrive,
        url: String,
        fid: String,
        cache: ChunkCache,
        size: u64,
        pos: u64,
    ) -> Self {
        let mut p = Self {
            drive,
            url,
            fid,
            cache,
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
            let cache = self.cache.clone();
            let key = (self.fid.clone(), index);
            /*
                缓存查询放在**任务内部**，不在这里同步查。

                这样窗口的结构完全不变——命中与否都是「一个会 resolve 出 Bytes 的
                任务」，调用方不需要分两种情况。命中时这个任务几乎立即完成，代价只是
                一次 spawn。
            */
            let task = tokio::spawn(async move {
                /*
                    `try_get_with` 而不是「先 get 再 insert」。

                    差别是**单飞**：同一个 key 同时被多个任务要时，moka 只让一个真的
                    去下载，其余的等它的结果。

                    这一条是实测逼出来的。先 get 再 insert 的版本只把放大比从 2.41×
                    降到 2.16×，因为它只挡得住**已完成**的块——而播放器换连接的时机
                    恰恰是上一个窗口预读的块**还在途**的时候，于是新窗口对同一批块
                    又下了一遍。缓存里没有，就都以为该自己去拉。
                */
                let res = cache
                    .try_get_with(key, async move {
                        // 名额在这里要，不在外面：命中或搭车的任务根本不打网络，
                        // 不该占坑。
                        let _permit = slots()
                            .acquire()
                            .await
                            .map_err(|e| anyhow::anyhow!("semaphore closed: {e}"))?;
                        drive.download(url, Some((start, len))).await
                    })
                    .await;
                match res {
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
    /// 窗口被丢弃时**不动在途任务**，让它们跑完。
    ///
    /// 这一版和最初的写法相反，是被数据推翻的：最初这里 abort 掉所有在途请求，
    /// 理由是「别给夸克堆连接」。听起来对，实测却是最贵的一种做法——
    ///
    /// **被预读的块正好就是连接关闭时还在途的那几块。** 播放器每十几秒换一条连接
    /// （dav-server 每个请求新建一个 DavFile），abort 等于每次都把最该留下的数据
    /// 扔掉。加了共享缓存也只把放大比从 2.41× 压到 2.16×，因为能进缓存的只有
    /// 已经跑完的块。
    ///
    /// 改成让它们跑完之后，那几块会进缓存，下一条连接直接命中。连接数的兜底交给
    /// `DOWNLOAD_SLOTS`——用信号量限并发，比用 abort 限并发精确得多，也不会误伤
    /// 马上就要用到的数据。
    fn drop(&mut self) {
        // 故意不 abort：让它们跑完，结果进共享缓存。
        //
        // 最初这里是 abort，实测证明那是错的：被预读的块**正好就是**连接关闭时还在
        // 途的那几块，abort 掉等于把最该留下的数据扔了。加了缓存也只降 10%
        // （2.41× → 2.16×），因为进缓存的只有已完成的块。
        //
        // 放它们跑完之后，下一条连接（播放器十几秒就换一条）能直接命中。并发的
        // 兜底交给 DOWNLOAD_SLOTS，不靠 abort。
    }
}
