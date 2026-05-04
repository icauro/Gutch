# What Gutch Is

Gutch is a storage and delivery system for versioned product builds.

At a high level, Gutch keeps:

- installable target manifests
- reusable stored data bundles

The practical idea is simple. If version `1.0.1` is mostly the same as version `1.0.0`, Gutch should not have to behave as if the entire product is brand new. It should be able to recognize what stayed the same, reuse that stored data, and only add what actually changed.

That is why product structure matters. Gutch is much more effective when releases preserve stable file names, stable grouping, and stable packaging rules.

## Why Not Just Ship Zips

The obvious alternative is to upload one compressed build per release. That works, but it has a ceiling.

If you publish full zipped or compressed builds for every version, small changes often still force large uploads. Long-term retention gets expensive because unchanged data is stored repeatedly. Repair also tends to collapse into a reinstall mindset, because the unit of recovery is usually the whole archive rather than the specific content that drifted.

This gets worse when a release process rebuilds giant archives for tiny changes. One modified map, one language update, or one small asset tweak can make a full package look new even when most of the product is unchanged.

## Why Not Just Ship Patch Chains

Traditional patch chains solve a different problem. They are good at moving a user from one known version to the next. They are less good when you want to retain many versions, recover from local drift, or reconstruct old states reliably over time.

Patch chains tend to assume a specific starting point. If the local install is missing files, hand-modified, or otherwise not what the patch expected, recovery gets harder. Retaining many versions also becomes operationally awkward, because history is tied to patch relationships rather than to independently reconstructable stored content.

## Where Gutch Is Better

Gutch is a better fit when you want all of these at once:

- many retained versions
- reuse across releases
- clean optional-content boundaries
- reconstruction of specific target states
- safer repair

It is not magic. If your packaging rewrites everything every release, reuse will still suffer. But if your product structure is reasonably stable, Gutch can preserve much more reuse than a "one zip per release" workflow and a much cleaner recovery story than a fragile patch chain.

## A Useful Git Analogy

Gutch is not Git, but the comparison is useful at a surface level.

Both systems benefit when unchanged content stays unchanged. Both care about content identity. Both are easier to reason about when history is retained instead of treated as a pile of unrelated snapshots.

The analogy stops quickly, though. Git is for source collaboration. Gutch is for published product outputs. Git is not your installer, updater, or repair engine. Gutch sits much closer to those workflows.

The most useful borrowed intuition is this: stable content pays off. When unchanged material remains stable across releases, the system can reuse it instead of rediscovering everything from scratch.

## The Main Rule

Gutch rewards stable content.

That sounds obvious, but it has real consequences. If a release process repacks all files, changes archive ordering randomly, embeds fresh timestamps everywhere, or otherwise rewrites unrelated bytes, Gutch sees less reusable structure. If packaging is stable, reuse improves. Uploads get smaller. Repair gets narrower. Retaining many versions becomes less painful.
