// The socket to the compositor, and the frames on it.

using System;
using System.Buffers.Binary;
using System.Net.Sockets;

namespace Solium.Wm;

/// <summary>
/// The connection Solium handed this process: its standard input, which is
/// a Unix stream socket. Each message is one frame: a 32-bit byte count,
/// little-endian, and then that many bytes.
/// </summary>
public sealed class Connection : IDisposable
{
    private readonly Socket _socket;

    private Connection(Socket socket) => _socket = socket;

    public static Connection FromStandardInput()
    {
        // File descriptor 0. Not owned: closing it is the process ending.
        var handle = new SafeSocketHandle((IntPtr)0, ownsHandle: false);
        return new Connection(new Socket(handle));
    }

    public void Send(Message message)
    {
        var writer = new Writer();
        message.Encode(writer);
        var body = writer.ToArray();
        var frame = new byte[4 + body.Length];
        BinaryPrimitives.WriteUInt32LittleEndian(frame, (uint)body.Length);
        body.CopyTo(frame, 4);
        var sent = 0;
        while (sent < frame.Length)
        {
            sent += _socket.Send(frame, sent, frame.Length - sent, SocketFlags.None);
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
        // A frame that has begun arrives whole in a moment: read all of it.
        var header = new byte[4];
        if (!Fill(header))
        {
            return new Message.Goodbye { Value = "the compositor closed the connection" };
        }
        var body = new byte[BinaryPrimitives.ReadUInt32LittleEndian(header)];
        if (!Fill(body))
        {
            return new Message.Goodbye { Value = "the compositor closed the connection" };
        }
        return Message.Decode(new Reader(body));
    }

    private bool Fill(byte[] buffer)
    {
        var read = 0;
        while (read < buffer.Length)
        {
            var count = _socket.Receive(buffer, read, buffer.Length - read, SocketFlags.None);
            if (count == 0)
            {
                return false;
            }
            read += count;
        }
        return true;
    }

    public void Dispose() => _socket.Dispose();
}
