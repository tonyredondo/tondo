// The C# side of the rows where LINQ says something: the same work as the
// Tondo benchmark of the same name in ../tondo, measured as `go test -bench`
// measures Go's, and written as `go test` writes it, so that compare.to
// reads both the same way: `BenchmarkQueries/groupBy_10000  N  t ns/op  b B/op`.
using System.Diagnostics;

record Order(int Id, int Customer, int Cents, string Region);
record Customer(int Id, string Name);

static class Program
{
    static object? sink;

    static List<Order> Orders(int count)
    {
        string[] regions = ["north", "south", "east", "west"];
        var all = new List<Order>();
        long seed = 42;
        for (var id = 0; id < count; id++)
        {
            seed = (seed * 1_103_515_245 + 12345) % 2_147_483_648;
            all.Add(new Order(id, (int)(seed % 1000), (int)(seed % 100_000), regions[seed % 4]));
        }
        return all;
    }

    // Runs `body` for about `seconds`, after as long warming up, and writes
    // its time and bytes allocated for each run.
    static void Bench(string name, double seconds, Func<object?> body)
    {
        var warm = Stopwatch.StartNew();
        long runs = 0;
        while (warm.Elapsed.TotalSeconds < seconds) { sink = body(); runs++; }
        var count = Math.Max(1, runs);
        var before = GC.GetAllocatedBytesForCurrentThread();
        var clock = Stopwatch.StartNew();
        for (long i = 0; i < count; i++) sink = body();
        var elapsed = clock.Elapsed.TotalNanoseconds;
        var bytes = GC.GetAllocatedBytesForCurrentThread() - before;
        Console.WriteLine($"Benchmark{name.Replace(' ', '_')}\t{count}\t{elapsed / count:F1} ns/op\t{bytes / count} B/op");
    }

    static void Main(string[] args)
    {
        var seconds = args.Length > 0 ? double.Parse(args[0]) : 1.0;
        var all = Orders(10_000);
        var amounts = all.Select(o => o.Cents).ToArray();
        Bench("Queries/numbers map filter sum 10000", seconds,
            () => amounts.Select(n => n * 2).ToArray().Where(n => n % 3 == 0).ToArray().Sum());
        Bench("Queries/numbers iter map filter sum 10000", seconds,
            () => amounts.Select(n => n * 2).Where(n => n % 3 == 0).Sum());
        Bench("Queries/filter sumBy 10000", seconds,
            () => all.Where(o => o.Cents > 50_000).ToArray().Sum(o => o.Cents));
        Bench("Queries/iter filter sumBy 10000", seconds,
            () => all.Where(o => o.Cents > 50_000).Sum(o => o.Cents));
        Bench("Queries/groupBy 10000", seconds,
            () => all.GroupBy(o => o.Customer).ToDictionary(g => g.Key, g => g.ToArray()).Count);
        Bench("Queries/sortedBy 10000", seconds, () => all.OrderBy(o => o.Cents).ToArray());
        Bench("Queries/countBy 10000", seconds, () => all.CountBy(o => o.Region).ToDictionary());
        Bench("Queries/maxBy 10000", seconds, () => all.MaxBy(o => o.Cents));
        Bench("Queries/keyedBy 10000", seconds, () => all.ToDictionary(o => o.Id));
        Bench("Queries/uniqueBy 10000", seconds, () => all.DistinctBy(o => o.Customer).ToArray());
        Bench("Queries/averageBy 10000", seconds, () => all.Average(o => (double)o.Cents));
        Bench("Queries/toSet 10000", seconds, () => amounts.ToHashSet());
        var people = Enumerable.Range(0, 1000).Select(id => new Customer(id, $"customer {id}")).ToArray();
        Bench("Queries/joinBy 10000x1000", seconds,
            () => all.Join(people, o => o.Customer, p => p.Id, (o, p) => (o, p)).Count());
    }
}
