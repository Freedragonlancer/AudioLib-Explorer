// tools/remote-resume-fixture.cs
// Loopback-only fault injection for validate-remote-resume.ps1. No user data or remote hosts.
using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

public sealed class AudioLibResumeRequest
{
    public long Offset;
    public string Range;
    public string IfRange;
    public string SnapshotId;
    public int Sent;
}

public sealed class AudioLibResumeScenario
{
    public readonly byte[] Body;
    public readonly Dictionary<string, string> Headers;
    public readonly ConcurrentQueue<AudioLibResumeRequest> Requests = new ConcurrentQueue<AudioLibResumeRequest>();
    public volatile string Library;
    public int CutsLeft;
    public int StallsLeft;
    public volatile bool Stall;
    public int Stalled;

    public AudioLibResumeScenario(byte[] body, Dictionary<string, string> headers, string library)
    {
        Body = body;
        Headers = new Dictionary<string, string>(headers, StringComparer.OrdinalIgnoreCase);
        Library = library;
    }

    internal bool TakeCut()
    {
        while (true)
        {
            int value = Volatile.Read(ref CutsLeft);
            if (value <= 0) return false;
            if (Interlocked.CompareExchange(ref CutsLeft, value - 1, value) == value) return true;
        }
    }

    internal bool TakeStall()
    {
        while (true)
        {
            int value = Volatile.Read(ref StallsLeft);
            if (value <= 0) return false;
            if (Interlocked.CompareExchange(ref StallsLeft, value - 1, value) == value) return true;
        }
    }
}

public sealed class AudioLibResumeFixture : IDisposable
{
    private readonly TcpListener listener = new TcpListener(IPAddress.Loopback, 0);
    private readonly ConcurrentDictionary<string, AudioLibResumeScenario> scenarios = new ConcurrentDictionary<string, AudioLibResumeScenario>();
    private readonly ConcurrentDictionary<TcpClient, byte> clients = new ConcurrentDictionary<TcpClient, byte>();
    private readonly ConcurrentBag<Task> handlers = new ConcurrentBag<Task>();
    public readonly ConcurrentQueue<string> Errors = new ConcurrentQueue<string>();
    private readonly Task accepting;
    private readonly byte[] health;
    private readonly byte[] roots;
    private volatile bool stopping;
    public readonly int Port;

    public AudioLibResumeFixture(string healthJson, string rootsJson)
    {
        health = Encoding.UTF8.GetBytes(healthJson);
        roots = Encoding.UTF8.GetBytes(rootsJson);
        listener.Start();
        Port = ((IPEndPoint)listener.LocalEndpoint).Port;
        accepting = Task.Run((Action)Accept);
    }

    public void Add(string name, AudioLibResumeScenario scenario) { scenarios[name] = scenario; }

    private void Accept()
    {
        while (!stopping)
        {
            TcpClient client;
            try { client = listener.AcceptTcpClient(); }
            catch (SocketException) { if (stopping) return; throw; }
            catch (ObjectDisposedException) { return; }
            clients[client] = 0;
            handlers.Add(Task.Run(() => Handle(client)));
        }
    }

    private static void Head(Stream output, int status, long length, Dictionary<string, string> extra)
    {
        var text = new StringBuilder("HTTP/1.1 " + status + " " + (status == 206 ? "Partial Content" : status == 200 ? "OK" : "Error") + "\r\n");
        text.Append("Connection: close\r\nContent-Length: ").Append(length).Append("\r\n");
        foreach (var header in extra) text.Append(header.Key).Append(": ").Append(header.Value).Append("\r\n");
        text.Append("\r\n");
        byte[] bytes = Encoding.ASCII.GetBytes(text.ToString());
        output.Write(bytes, 0, bytes.Length);
    }

    private static void Json(Stream output, int status, byte[] body)
    {
        Head(output, status, body.Length, new Dictionary<string, string> { { "Content-Type", "application/json" } });
        output.Write(body, 0, body.Length);
    }

    private void Handle(TcpClient client)
    {
        try
        {
            client.ReceiveTimeout = 10000;
            client.SendTimeout = 5000;
            using (NetworkStream output = client.GetStream())
            using (var reader = new StreamReader(output, Encoding.ASCII, false, 1024, true))
            {
                string line = reader.ReadLine();
                if (String.IsNullOrEmpty(line)) return;
                string[] first = line.Split(' ');
                if (first.Length < 2) return;
                var uri = new Uri("http://127.0.0.1" + first[1]);
                string[] segments = uri.AbsolutePath.Split(new[] { '/' }, StringSplitOptions.RemoveEmptyEntries);
                AudioLibResumeScenario scenario;
                if (segments.Length < 4 || !scenarios.TryGetValue(segments[0], out scenario))
                {
                    Json(output, 404, Encoding.UTF8.GetBytes("{}"));
                    return;
                }
                var headers = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
                while (!String.IsNullOrEmpty(line = reader.ReadLine()))
                {
                    int split = line.IndexOf(':');
                    if (split > 0) headers[line.Substring(0, split).Trim()] = line.Substring(split + 1).Trim();
                }
                string path = "/" + String.Join("/", segments, 3, segments.Length - 3);
                if (path == "/health") { Json(output, 200, health); return; }
                if (path == "/library") { Json(output, 200, Encoding.UTF8.GetBytes(scenario.Library)); return; }
                if (path == "/roots") { Json(output, 200, roots); return; }
                if (path != "/library/snapshot") { Json(output, 404, Encoding.UTF8.GetBytes("{}")); return; }
                string snapshotId = null;
                foreach (string parameter in uri.Query.TrimStart('?').Split('&'))
                {
                    if (parameter.StartsWith("snapshot_id=", StringComparison.Ordinal)) snapshotId = Uri.UnescapeDataString(parameter.Substring(12));
                }
                if (snapshotId != null && snapshotId != scenario.Headers["X-Snapshot-Id"])
                {
                    Json(output, 410, Encoding.UTF8.GetBytes("{\"error_code\":\"SNAPSHOT_EXPIRED\",\"message\":\"Synthetic snapshot expired\"}"));
                    return;
                }
                string range, ifRange;
                headers.TryGetValue("Range", out range);
                headers.TryGetValue("If-Range", out ifRange);
                long offset = 0;
                bool partial = range != null && ifRange == scenario.Headers["ETag"];
                if (partial && (!range.StartsWith("bytes=", StringComparison.Ordinal) || !range.EndsWith("-", StringComparison.Ordinal) || !Int64.TryParse(range.Substring(6, range.Length - 7), out offset)))
                {
                    Json(output, 400, Encoding.UTF8.GetBytes("{}")); return;
                }
                if (offset < 0 || offset >= scenario.Body.LongLength)
                {
                    Head(output, 416, 0, new Dictionary<string, string> { { "Content-Range", "bytes */" + scenario.Body.Length } }); return;
                }
                var reply = new Dictionary<string, string>(scenario.Headers, StringComparer.OrdinalIgnoreCase);
                reply["Accept-Ranges"] = "bytes";
                if (partial) reply["Content-Range"] = "bytes " + offset + "-" + (scenario.Body.Length - 1) + "/" + scenario.Body.Length;
                var request = new AudioLibResumeRequest { Offset = offset, Range = range, IfRange = ifRange, SnapshotId = snapshotId };
                scenario.Requests.Enqueue(request);
                int remaining = scenario.Body.Length - (int)offset;
                bool cut = scenario.TakeCut();
                bool oneShotStall = scenario.TakeStall();
                bool stall = scenario.Stall || oneShotStall;
                int count = cut || stall ? Math.Max(1, remaining / 2) : remaining;
                Head(output, partial ? 206 : 200, remaining, reply);
                output.Write(scenario.Body, (int)offset, count);
                output.Flush();
                Volatile.Write(ref request.Sent, count);
                if (stall)
                {
                    Interlocked.Increment(ref scenario.Stalled);
                    while (!stopping && (scenario.Stall || oneShotStall))
                    {
                        if (client.Client.Poll(0, SelectMode.SelectRead) && client.Available == 0) break;
                        Thread.Sleep(25);
                    }
                    if (!stopping && !scenario.Stall && !oneShotStall) output.Write(scenario.Body, (int)offset + count, remaining - count);
                }
            }
        }
        catch (IOException) { }
        catch (SocketException) { }
        catch (ObjectDisposedException) { }
        catch (Exception error) { Errors.Enqueue(error.GetType().Name + ": " + error.Message); }
        finally { byte ignored; clients.TryRemove(client, out ignored); client.Dispose(); }
    }

    public void Dispose()
    {
        stopping = true;
        listener.Stop();
        foreach (var client in clients.Keys) client.Dispose();
        accepting.GetAwaiter().GetResult();
        Task.WaitAll(handlers.ToArray(), 5000);
    }
}

public static class AudioLibResumeWindow
{
    [DllImport("user32.dll", SetLastError = true)] public static extern bool PostMessage(IntPtr window, uint message, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
}
