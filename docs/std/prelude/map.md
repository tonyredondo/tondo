# Map

`Map[K, V]` keeps its entries in the order they were added.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api Map` shows the same in a terminal.

- [Map](#map): [`Map.mapValues`](#mapmapvalues), [`Map.filter`](#mapfilter), [`Map.merge`](#mapmerge), [`Map.length`](#maplength), [`Map.get`](#mapget), [`Map.contains`](#mapcontains), [`Map.insert`](#mapinsert), [`Map.remove`](#mapremove), [`Map.clear`](#mapclear), [`Map.keys`](#mapkeys), [`Map.values`](#mapvalues), [`Map.entries`](#mapentries)

### Map

#### Map.mapValues

```tondo
pub fn Map[K: Key, V].mapValues[W](self, transform: fn(V): W): Map[K, W]
```

The same keys, each with the function's result for its value.

#### Map.filter

```tondo
pub fn Map[K: Key, V].filter(self, keep: fn(K, V): Bool): Map[K, V]
```

The entries that pass the test, in the same order.

#### Map.merge

```tondo
pub fn Map[K: Key, V].merge(self, other: Map[K, V]): Map[K, V]
```

The entries of both, with those of `other` where both have a key.

#### Map.length

```tondo
pub fn Map.length(self): Int
```

The number of entries.

#### Map.get

```tondo
pub fn Map.get(self, key: K): V?
```

The value of `key`, or `none` when the map does not have it.

#### Map.contains

```tondo
pub fn Map.contains(self, key: K): Bool
```

Whether the map has `key`.

#### Map.insert

```tondo
pub fn Map.insert(var self, key: K, value: V): V?
```

`map[key] = value` inserts or replaces.

#### Map.remove

```tondo
pub fn Map.remove(var self, key: K): V?
```

Removes `key` and gives its value, or `none` when the map did not have
it.

#### Map.clear

```tondo
pub fn Map.clear(var self)
```

Removes every entry.

#### Map.keys

```tondo
pub fn Map.keys(self): MapKeys[K, V]
```

The keys, as an iterator in insertion order: `collect()` makes an array of them.

#### Map.values

```tondo
pub fn Map.values(self): MapValues[K, V]
```

The values, as an iterator in insertion order: `collect()` makes an array of them.

#### Map.entries

```tondo
pub fn Map.entries(self): MapEntries[K, V]
```

Each key with its value, as an iterator in insertion order: `collect()` makes an array
of them.
