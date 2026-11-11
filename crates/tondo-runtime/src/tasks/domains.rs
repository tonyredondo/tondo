//! The domains of the machine's cores, those that share a last-level cache:
//! a task that a worker of another domain takes runs away from the data its
//! starter left in the cache, which costs a crossing between the chips of a
//! processor, as the two of a Ryzen 9 5950X. Workers sleep in the domain of
//! the core they last ran on, a worker woken for tasks is one of the domain
//! of the core that made them, and tasks wait longer before a worker of
//! another domain takes them. No worker is bound to its cores: a worker in
//! a domain is one the system ran there last, and keeps there, as Linux
//! wakes a thread near where it ran. One domain on a machine with a cache
//! for every core, or that says nothing of its caches, or with
//! `TONDO_CACHE_DOMAINS=0`.

use std::sync::OnceLock;

/// The most domains told apart; a machine with more is one domain.
pub(crate) const MOST: usize = 16;

struct Map {
    /// The domain of each core, by its number.
    of: Vec<u8>,
    count: usize,
}

fn map() -> &'static Map {
    static MAP: OnceLock<Map> = OnceLock::new();
    MAP.get_or_init(|| read().unwrap_or(Map { of: Vec::new(), count: 1 }))
}

/// The cores of each last-level cache, as Linux lists them.
#[cfg(target_os = "linux")]
fn read() -> Option<Map> {
    if crate::config::config().cache_domains.as_deref().is_some_and(|value| value.trim() == "0") {
        return None;
    }
    let mut of: Vec<u8> = Vec::new();
    let mut lists: Vec<String> = Vec::new();
    for cpu in 0.. {
        let base = format!("/sys/devices/system/cpu/cpu{cpu}");
        if !std::path::Path::new(&base).exists() {
            break;
        }
        // The cache of the highest level the core has.
        let mut shared = None;
        let mut level = 0;
        for index in 0..8 {
            let cache = format!("{base}/cache/index{index}");
            let Ok(this) = std::fs::read_to_string(format!("{cache}/level")) else { break };
            let this: u32 = this.trim().parse().ok()?;
            if this >= level {
                level = this;
                shared = std::fs::read_to_string(format!("{cache}/shared_cpu_list")).ok();
            }
        }
        let shared = shared?.trim().to_string();
        let domain = match lists.iter().position(|known| *known == shared) {
            Some(domain) => domain,
            None => {
                lists.push(shared);
                lists.len() - 1
            }
        };
        if domain >= MOST {
            return None;
        }
        of.push(domain as u8);
    }
    (lists.len() > 1).then_some(Map { of, count: lists.len() })
}

#[cfg(not(target_os = "linux"))]
fn read() -> Option<Map> {
    None
}

/// How many domains there are; 1 when they are not told apart.
pub(crate) fn count() -> usize {
    map().count
}

/// The domain of the core the thread runs on now.
pub(crate) fn here() -> usize {
    let map = map();
    if map.count == 1 {
        return 0;
    }
    #[cfg(target_os = "linux")]
    {
        let cpu = unsafe { libc::sched_getcpu() };
        if cpu >= 0
            && let Some(&domain) = map.of.get(cpu as usize)
        {
            return domain as usize;
        }
    }
    0
}
