// The film: each frame as the rectangles that are lit in it.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;

namespace Solium.Wm;

/// <summary>A rectangle of a frame, in cells of the film's grid.</summary>
public readonly record struct Cells(int X, int Y, int W, int H)
{
    public double CentreX => X + W / 2.0;
    public double CentreY => Y + H / 2.0;
}

/// <summary>
/// What <c>frames.py</c> writes: a few lines saying the grid, the frames a
/// second and the most rectangles any frame has, and then a line a frame,
/// its rectangles as <c>x y w h</c> separated by <c>;</c>.
/// </summary>
public sealed class Film
{
    public int Columns { get; private init; }
    public int Rows { get; private init; }
    public double FramesASecond { get; private init; }

    /// <summary>The most rectangles a frame has: how many windows it takes.</summary>
    public int Most { get; private init; }

    public List<Cells[]> Frames { get; } = new();

    public static Film Load(string path)
    {
        using var reader = new StreamReader(path);
        int columns = 0, rows = 0, most = 0;
        double rate = 0;
        string? line;
        // The header ends at the line that says how many frames follow.
        while ((line = reader.ReadLine()) is not null)
        {
            var words = line.Split(' ', StringSplitOptions.RemoveEmptyEntries);
            if (words.Length == 0)
            {
                continue;
            }
            if (words[0] == "grid")
            {
                columns = int.Parse(words[1], CultureInfo.InvariantCulture);
                rows = int.Parse(words[2], CultureInfo.InvariantCulture);
            }
            else if (words[0] == "fps")
            {
                rate = double.Parse(words[1], CultureInfo.InvariantCulture);
            }
            else if (words[0] == "most")
            {
                most = int.Parse(words[1], CultureInfo.InvariantCulture);
            }
            else if (words[0] == "frames")
            {
                break;
            }
        }
        if (columns <= 0 || rows <= 0 || rate <= 0 || most <= 0)
        {
            throw new InvalidDataException($"{path} is not a film frames.py wrote");
        }
        var film = new Film { Columns = columns, Rows = rows, FramesASecond = rate, Most = most };
        while ((line = reader.ReadLine()) is not null)
        {
            var lit = new List<Cells>();
            foreach (var each in line.Split(';', StringSplitOptions.RemoveEmptyEntries))
            {
                var n = each.Split(' ', StringSplitOptions.RemoveEmptyEntries);
                lit.Add(new Cells(
                    int.Parse(n[0], CultureInfo.InvariantCulture),
                    int.Parse(n[1], CultureInfo.InvariantCulture),
                    int.Parse(n[2], CultureInfo.InvariantCulture),
                    int.Parse(n[3], CultureInfo.InvariantCulture)));
            }
            film.Frames.Add(lit.ToArray());
        }
        if (film.Frames.Count == 0)
        {
            throw new InvalidDataException($"{path} has no frames");
        }
        return film;
    }
}
