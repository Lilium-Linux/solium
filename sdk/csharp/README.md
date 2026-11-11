# A window manager for Solium in C#

`Solium.Wm` is what a window manager in C# is built on: the protocol's
types, the connection to the compositor, and the conversation kept for you.
[`Tiny`](Tiny/Program.cs) is a whole window manager on it, in about a hundred
lines, and [`examples/bad-apple`](../../examples/bad-apple/README.md) is a
larger one.

```csharp
using System.Collections.Generic;
using Solium.Wm;

internal sealed class Mine : WindowManager
{
    // Before the first frame, and again at a reload: its keys, and the
    // events it wants to hear.
    protected override IEnumerable<Command> Setup() => new[]
    {
        Commands.Bind(("super+return", "open a terminal")),
        Commands.Listen("opened", "closed"),
    };

    // One batch. `Desktop` already holds what changed with it.
    protected override void Handle(Batch batch, Answer answer)
    {
        foreach (var each in batch.Events)
        {
            if (each.KindCase == Event.KindOneofCase.Key && each.Key == "super+return")
            {
                answer.Handled = true;
                answer.Commands.Add(Commands.Spawn(new[] { "foot" }));
            }
        }
    }
}
```

`new Mine().Run()` from `Main` is the rest. Build it, and name it in
`~/.config/solium/wm.toml`:

```toml
command = ["dotnet", "/path/to/mine.dll"]
```

[Writing a window manager](../../docs/window-managers.md) says what the
compositor tells a window manager and what it may ask for; this page is only
how that reads in C#.

## What is in it

| | |
|---|---|
| The protocol's types | `Command`, `Event`, `Window`, `Place` and the rest. They are not written here: protobuf's own generator writes them from [`solium_wm.proto`](../../crates/wm-protocol/proto/solium_wm.proto) when the project is built. |
| `WindowManager` | The conversation: the handshake, every batch answered once and in order, a reload, the goodbye. You write `Setup` and `Handle`. `Changing` shows a change before it is taken, `Submit` sends commands no batch asked for, and `Ask` asks the compositor a question. |
| `Desktop` | Your copy of the desktop, kept current from each batch before `Handle` is called: `Windows`, `Stacking`, `Monitors`, `WorkArea`, `Focused` and the rest. |
| `Commands` | The commands written most, each in one call: `Bind`, `Listen`, `Configure`, `Place`, `Focus`, `Close`, `Spawn`, `Every`. Every other command is written as its type has it. |
| `Document` | A settings section from an object: `Commands.Configure("idle", new { lock_after = 0 })`. |
| `Connection` | The socket and its frames, for a program that wants to keep the conversation itself. |

## How the types read

A variant of the protocol is a class with one property set, and `KindCase`
says which:

```csharp
switch (each.KindCase)
{
    case Event.KindOneofCase.Opened:        // each.Opened is the window's id
    case Event.KindOneofCase.Fullscreen:    // each.Fullscreen.Id, .Entering
}

var place = new Command { Place = new Place { Id = 7, Rect = rect, Tile = true } };
var quit = new Command { Quit = new Unit() };   // a case that carries nothing
```

A field you do not set is not sent, and reads at the other end as its
default: a `Place` with no `Motion` does not animate. A list that is a
variant's case is a small class with `Items`
(`new Command { Spawn = new StringList { Items = { "foot" } } }`), and a
value that may be absent is one with `Value`.

Two of the protocol's names are also names in `System.Threading`: `Monitor`
and `Timer`. A file that uses both namespaces says which it means:

```csharp
using Monitor = Solium.Wm.Monitor;
```

## Building

```sh
dotnet build sdk/csharp/Tiny -c Release -o /tmp/tiny
```

It needs the .NET SDK, 8 or later, and two packages from NuGet, which
`dotnet` fetches by itself: `Google.Protobuf`, and `Grpc.Tools` for the
generator. Reference the package from a project of your own with:

```xml
<ProjectReference Include="path/to/sdk/csharp/Solium.Wm/Solium.Wm.csproj" />
```
