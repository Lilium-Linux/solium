// The socket to the compositor, and the frames on it.

using System;
using System.IO;
using System.Net.Sockets;
using Google.Protobuf;

namespace Solium.Wm;

/// <summary>
/// The connection Solium handed this process: its standard input, which is
/// a Unix stream socket, used both ways. Each message on it is one frame:
/// its length as a varint, and then the message, which is protobuf's own
/// delimited framing.
/// </summary>
public sealed class Connection : IDisposable
{
    private readonly Socket _socket;
    private readonly NetworkStream _stream;

    private Connection(Socket socket)
    {
        _socket = socket;
        _stream = new NetworkStream(socket, ownsSocket: false);
    }

    public static Connection FromStandardInput()
    {
        // File descriptor 0. Not owned: closing it is the process ending.
        var handle = new SafeSocketHandle((IntPtr)0, ownsHandle: false);
        return new Connection(new Socket(handle));
    }

    /// <summary>Send one message, whole.</summary>
    public void Send(Message message)
    {
        // Written to memory first, so the frame is one write on the socket
        // and not one for its length and another for the rest.
        using var frame = new MemoryStream();
        message.WriteDelimitedTo(frame);
        var bytes = frame.GetBuffer();
        var length = (int)frame.Length;
        var sent = 0;
        while (sent < length)
        {
            sent += _socket.Send(bytes, sent, length - sent, SocketFlags.None);
        }
    }

    /// <summary>
    /// The compositor's next message, waited for as long as
    /// <paramref name="patience"/>: null when nothing came in that time, and
    /// when the compositor closed the connection, a goodbye saying so.
    /// </summary>
    public Message? Receive(TimeSpan patience)
    {
        var micros = (int)Math.Clamp(patience.TotalMicroseconds, 0, int.MaxValue);
        if (!_socket.Poll(micros, SelectMode.SelectRead))
        {
            return null;
        }
        // Readable with nothing to read is a socket the other side closed.
        if (_socket.Available == 0)
        {
            return new Message { Goodbye = "the compositor closed the connection" };
        }
        // A frame that has begun arrives whole in a moment: read all of it.
        return Message.Parser.ParseDelimitedFrom(_stream);
    }

    public void Dispose() => _stream.Dispose();
}
