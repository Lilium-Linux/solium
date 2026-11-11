// A window manager: the conversation with the compositor, kept for it.

using System;
using System.Collections.Generic;

namespace Solium.Wm;

/// <summary>
/// A window manager for Solium. Derive from it, say what it asks for in
/// <see cref="Setup"/> and what it does in <see cref="Handle"/>, and call
/// <see cref="Run()"/> from <c>Main</c>.
///
/// The rest of the conversation is kept here: the handshake, the desktop
/// kept current from each batch (<see cref="Desktop"/>), every batch
/// answered once and in order, a reload answered with the setup again, and
/// the goodbye that ends it.
/// </summary>
public abstract class WindowManager
{
    private Connection? _solium;
    private ulong _questions;
    private readonly Queue<Message> _waiting = new();

    /// <summary>The desktop as the compositor last told it.</summary>
    protected Desktop Desktop { get; } = new();

    /// <summary>
    /// What the window manager asks for before the first frame, and again
    /// when it is told to reload: the keys that are its own, the events it
    /// listens to, its settings.
    /// </summary>
    protected abstract IEnumerable<Command> Setup();

    /// <summary>
    /// One batch. <see cref="Desktop"/> already holds what changed with it.
    /// Add the commands it comes to to <paramref name="answer"/>, and set
    /// its <c>Handled</c> when the batch's key, press or scroll was this
    /// window manager's, or when it placed the window that opened.
    /// </summary>
    protected abstract void Handle(Batch batch, Answer answer);

    /// <summary>
    /// A change about to be taken into <see cref="Desktop"/>, which still
    /// holds what was: for a window manager that needs to see what a window
    /// was before it changed.
    /// </summary>
    protected virtual void Changing(Change change)
    {
    }

    /// <summary>
    /// Talk to the compositor on the socket it started this process with,
    /// until it says goodbye: the process's exit code.
    /// </summary>
    public int Run()
    {
        try
        {
            using var solium = Connection.FromStandardInput();
            return Run(solium);
        }
        catch (Exception error) when (error is System.Net.Sockets.SocketException or System.IO.IOException)
        {
            Console.Error.WriteLine($"the window manager lost the compositor: {error.Message}");
            return 1;
        }
    }

    /// <summary>The same, on a connection of the caller's.</summary>
    public int Run(Connection solium)
    {
        _solium = solium;
        if (solium.Receive(TimeSpan.FromSeconds(10)) is not { KindCase: Message.KindOneofCase.Hello } hello)
        {
            return 1;
        }
        if (!Protocol.Speaks(hello.Hello.Version))
        {
            solium.Send(new Message
            {
                Goodbye = $"this window manager speaks the protocol {Protocol.Version}, " +
                          $"the compositor {hello.Hello.Version}",
            });
            return 1;
        }
        solium.Send(new Message
        {
            HelloAnswer = new HelloAnswer { Version = Protocol.Version, Commands = { Setup() } },
        });
        while (true)
        {
            var message = _waiting.Count > 0 ? _waiting.Dequeue() : solium.Receive(TimeSpan.FromMinutes(1));
            switch (message?.KindCase)
            {
                case Message.KindOneofCase.Batch:
                    solium.Send(new Message { Answer = Answer(message.Batch) });
                    break;
                case Message.KindOneofCase.Reload:
                    solium.Send(new Message
                    {
                        Reloaded = new Reloaded { Loaded = new Answer { Commands = { Setup() } } },
                    });
                    break;
                case Message.KindOneofCase.Goodbye:
                    return 0;
                // Nothing came, or a message a newer compositor has and
                // this package does not: go on waiting.
                default:
                    break;
            }
        }
    }

    private Answer Answer(Batch batch)
    {
        var answer = new Answer { Serial = batch.Serial };
        // The changes first, so each event is handled with the desktop as
        // it was when it happened.
        foreach (var change in batch.Changes)
        {
            Changing(change);
            Desktop.Apply(change);
        }
        Handle(batch, answer);
        return answer;
    }

    /// <summary>
    /// Send commands of the window manager's own, asked for by no batch:
    /// from a thread's result, say. They are applied in the order they come.
    /// </summary>
    protected void Submit(IEnumerable<Command> commands)
    {
        Connected().Send(new Message { Submit = new CommandList { Items = { commands } } });
    }

    /// <summary>
    /// Ask the compositor something and wait for its reply. What it sends
    /// meanwhile is kept, and handled afterwards in its order.
    /// </summary>
    protected Reply Ask(Query query)
    {
        var solium = Connected();
        var serial = ++_questions;
        solium.Send(new Message { Query = new Asked { Serial = serial, Query = query } });
        while (true)
        {
            var message = solium.Receive(TimeSpan.FromSeconds(10))
                ?? throw new TimeoutException("the compositor did not reply to a query");
            if (message.KindCase == Message.KindOneofCase.Reply && message.Reply.Serial == serial)
            {
                return message.Reply.Reply;
            }
            _waiting.Enqueue(message);
            if (message.KindCase == Message.KindOneofCase.Goodbye)
            {
                throw new System.IO.IOException("the compositor said goodbye before it replied");
            }
        }
    }

    private Connection Connected() =>
        _solium ?? throw new InvalidOperationException("the window manager is not running yet");
}
