using System.Collections.Concurrent;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

var options = PromptedOptions.Parse(args);

var fixturesRoot = Path.Combine(AppContext.BaseDirectory, "..", "..", "..", "..", "fixtures");
fixturesRoot = Path.GetFullPath(fixturesRoot);
var gameDir = Path.Combine(fixturesRoot, options.Name);
var totalBytes = (long)options.SizeMB * 1024 * 1024;

if (Directory.Exists(gameDir))
{
    Directory.Delete(gameDir, recursive: true);
}

Console.WriteLine($"=== {options.Name} ===");
Console.WriteLine($"    Size: {options.SizeMB} MB, Versions: {options.Versions}, Change: {options.ChangePercent}%");

var layout = new[]
{
    new FileLayout("game.exe", 25, "."),
    new FileLayout(Path.Combine("audio", "eng.pak"), 20, "audio"),
    new FileLayout(Path.Combine("textures", "env.tex"), 15, "textures"),
    new FileLayout(Path.Combine("textures", "chr.tex"), 12, "textures"),
    new FileLayout(Path.Combine("data", "assets.pak"), 15, "data"),
    new FileLayout(Path.Combine("data", "config.json"), 8, "data"),
    new FileLayout("readme.txt", 5, "."),
};

var baseSeed = HashUtil.StableHash(options.Name);

var truthV1 = Path.Combine(gameDir, "truth", "v1");
CreateVersionDirectories(truthV1, layout);
Console.Write("    Generating v1...");
GenerateVersion(truthV1, layout, totalBytes, baseSeed);
Console.WriteLine(" done");

for (var version = 2; version <= options.Versions; version++)
{
    var truthVn = Path.Combine(gameDir, "truth", $"v{version}");
    CreateVersionDirectories(truthVn, layout);
    Console.Write($"    Generating v{version}...");

    var changed = 0;
    var unchanged = 0;

    Parallel.ForEach(layout, file =>
    {
        var shouldChange = file.ShouldChange(baseSeed, version, options.ChangePercent);
        var v1Path = Path.Combine(truthV1, file.Name);
        var vnPath = Path.Combine(truthVn, file.Name);

        if (shouldChange)
        {
            var fileSeed = HashUtil.StableHash($"{options.Name}|{version}|{file.Name}|{file.Dir}");
            GenerateFile(vnPath, file.SizeBytes(totalBytes), fileSeed);
            Interlocked.Increment(ref changed);
        }
        else
        {
            File.Copy(v1Path, vnPath, overwrite: true);
            Interlocked.Increment(ref unchanged);
        }
    });

    Console.WriteLine($" done ({changed} changed, {unchanged} unchanged)");
}

Directory.CreateDirectory(Path.Combine(gameDir, "artifacts"));
Directory.CreateDirectory(Path.Combine(gameDir, "applied"));

var fixtureMetadata = new FixtureMetadata
{
    Name = options.Name,
    ProductId = options.Name,
    SizeMB = options.SizeMB,
    Versions = options.Versions,
    ChangePercent = options.ChangePercent,
    Platform = "windows",
    Architecture = "x86_64",
    TierId = "base",
    VersionIds = Enumerable.Range(1, options.Versions).Select(v => $"{v}.0.0").ToArray(),
};
var metadataPath = Path.Combine(gameDir, "fixture.json");
File.WriteAllText(
    metadataPath,
    JsonSerializer.Serialize(fixtureMetadata, new JsonSerializerOptions { WriteIndented = true }),
    Encoding.UTF8
);

var v1Count = Directory.GetFiles(truthV1, "*", SearchOption.AllDirectories).Length;
var vLastCount = Directory.GetFiles(Path.Combine(gameDir, "truth", $"v{options.Versions}"), "*", SearchOption.AllDirectories).Length;
Console.WriteLine($"    Created: v1={v1Count} files, v{options.Versions}={vLastCount} files");
Console.WriteLine($"    Metadata: {metadataPath}");
Console.WriteLine($"    Path: {gameDir}");

static void CreateVersionDirectories(string versionDir, IReadOnlyCollection<FileLayout> layout)
{
    foreach (var file in layout)
    {
        Directory.CreateDirectory(Path.Combine(versionDir, file.Dir));
    }
}

static void GenerateVersion(string versionDir, IReadOnlyCollection<FileLayout> layout, long totalBytes, int baseSeed)
{
    Parallel.ForEach(layout, file =>
    {
        var seed = HashUtil.StableHash($"{baseSeed}|{file.Name}|{file.Dir}");
        GenerateFile(Path.Combine(versionDir, file.Name), file.SizeBytes(totalBytes), seed);
    });
}

static void GenerateFile(string path, int size, int seed)
{
    size = Math.Max(size, 10);
    var ext = Path.GetExtension(path).ToLowerInvariant();
    Directory.CreateDirectory(Path.GetDirectoryName(path)!);

    if (ext is ".json" or ".txt")
    {
        GenerateTextFile(path, size, seed, ext);
        return;
    }

    GenerateBinaryFile(path, size, seed);
}

static void GenerateBinaryFile(string path, int size, int seed)
{
    var rng = new Random(seed);
    var buffer = new byte[size];
    var position = 0;

    while (position < size)
    {
        var chunk = Math.Min(rng.Next(2048, 18432), size - position);
        var mode = rng.Next(0, 3);

        for (var i = 0; i < chunk; i++)
        {
            buffer[position + i] = mode switch
            {
                0 => (byte)rng.Next(0, 256),
                1 => (byte)Math.Floor(Math.Sin((i * 0.03) * Math.PI / 180) * 127 + 128),
                _ => (byte)(i % 4 switch { 0 => 0x00, 1 => 0xFF, 2 => 0xAA, _ => 0x55 }),
            };
        }

        position += chunk;
    }

    File.WriteAllBytes(path, buffer);
}

static void GenerateTextFile(string path, int size, int seed, string ext)
{
    var rng = new Random(seed);
    var sb = new StringBuilder(size + 64);

    if (ext == ".json")
    {
        sb.Append("{ \"data\": { ");
        while (sb.Length < size - 20)
        {
            sb.Append('"').Append('k').Append(rng.Next(0, 99999)).Append("\":")
                .Append(rng.Next(0, 9_999_999)).Append(',');
        }
        sb.Append("\"_end\":0 } }");
    }
    else
    {
        var words = new[] { "the", "game", "version", "update", "patch", "build", "file", "data", "config", "asset" };
        var index = 0;
        while (sb.Length < size)
        {
            sb.Append(words[index % words.Length]).Append(' ').Append(index).Append('\n');
            index++;
        }
    }

    File.WriteAllText(path, sb.ToString(), Encoding.UTF8);
}

static class HashUtil
{
    public static int StableHash(string value)
    {
        unchecked
        {
            var hash = 17;
            foreach (var c in value)
            {
                hash = hash * 31 + c;
            }
            return Math.Abs(hash == int.MinValue ? int.MaxValue : hash);
        }
    }
}

sealed record FileLayout(string Name, int Percent, string Dir)
{
    public int SizeBytes(long totalBytes)
    {
        var size = totalBytes * Percent / 100;
        size = Math.Max(10L, size);
        if (size > int.MaxValue)
        {
            throw new ArgumentOutOfRangeException(nameof(totalBytes), "Generated file size exceeds int.MaxValue.");
        }
        return (int)size;
    }

    public bool ShouldChange(int baseSeed, int version, int changePercent)
    {
        if (changePercent <= 0)
        {
            return false;
        }

        var seed = HashUtil.StableHash($"{baseSeed}|{version}|{Name}|{Dir}|change");
        return new Random(seed).Next(0, 100) < changePercent;
    }
}

sealed class PromptedOptions
{
    public required string Name { get; init; }
    public required int SizeMB { get; init; }
    public required int Versions { get; init; }
    public required int ChangePercent { get; init; }

    public static PromptedOptions Parse(string[] args)
    {
        string? name = null;
        int? sizeMb = null;
        int? versions = null;
        int? changePercent = null;

        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--name":
                case "-Name":
                    name = NextValue(args, ref i, "name");
                    break;
                case "--size-mb":
                case "-SizeMB":
                    sizeMb = int.Parse(NextValue(args, ref i, "size-mb"));
                    break;
                case "--versions":
                case "-Versions":
                    versions = int.Parse(NextValue(args, ref i, "versions"));
                    break;
                case "--change-percent":
                case "-ChangePercent":
                    changePercent = int.Parse(NextValue(args, ref i, "change-percent"));
                    break;
            }
        }

        name ??= Prompt("Fixture name", "mock-game-1");
        sizeMb ??= int.Parse(Prompt("Total size in MB", "4"));
        versions ??= int.Parse(Prompt("Version count", "2"));
        changePercent ??= int.Parse(Prompt("Percent changed between versions", "30"));

        if (sizeMb <= 0)
        {
            throw new ArgumentOutOfRangeException(nameof(sizeMb), "SizeMB must be > 0.");
        }
        if (versions <= 0)
        {
            throw new ArgumentOutOfRangeException(nameof(versions), "Versions must be > 0.");
        }
        if (changePercent is < 0 or > 100)
        {
            throw new ArgumentOutOfRangeException(nameof(changePercent), "ChangePercent must be 0..100.");
        }

        return new PromptedOptions
        {
            Name = name,
            SizeMB = sizeMb.Value,
            Versions = versions.Value,
            ChangePercent = changePercent.Value,
        };
    }

    private static string NextValue(string[] args, ref int index, string name)
    {
        if (index + 1 >= args.Length)
        {
            throw new ArgumentException($"Missing value for {name}.");
        }
        index++;
        return args[index];
    }

    private static string Prompt(string label, string fallback)
    {
        Console.Write($"{label} [{fallback}]: ");
        var value = Console.ReadLine();
        return string.IsNullOrWhiteSpace(value) ? fallback : value.Trim();
    }
}

sealed class FixtureMetadata
{
    public required string Name { get; init; }
    public required string ProductId { get; init; }
    public required int SizeMB { get; init; }
    public required int Versions { get; init; }
    public required int ChangePercent { get; init; }
    public required string Platform { get; init; }
    public required string Architecture { get; init; }
    public required string TierId { get; init; }
    public required string[] VersionIds { get; init; }
}
