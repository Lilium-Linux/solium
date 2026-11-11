// The film: each frame as the rectangles its light and its dark parts are
// covered with.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;

namespace Solium.Wm;

/// <summary>
/// One rectangle of a frame, in cells of the film's grid, and which
/// terminal of its colour it is: the same one for as long as it lives.
/// </summary>
public readonly record struct Box(int Terminal, int X, int Y, int W, int H);

/// <summary>A frame: the rectangles of each colour.</summary>
public sealed record Frame(Box[] Light, Box[] Dark);

/// <summary>
/// What <c>frames.py</c> writes: a few lines saying the grid, the frames a
/// second and how many terminals of each colour the film needs, and then a
/// line a frame: its light rectangles, a <c>|</c>, and its dark ones, each
/// <c>n x y w h</c> and separated by <c>;</c>.
/// </summary>
public sealed class Film
{
    public int Columns { get; private init; }
    public int Rows { get; private init; }
    public double FramesASecond { get; private init; }

    /// <summary>How many terminals of each colour the film needs at most.</summary>
    public int Light { get; private init; }
    public int Dark { get; private init; }

    public List<Frame> Frames { get; } = new();

    public static Film Load(string path)
    {
        using var reader = new StreamReader(path);
        int columns = 0, rows = 0, light = 0, dark = 0;
        double rate = 0;
        string? line;
        // The header ends at the line that says how many frames follow.
        while ((line = reader.ReadLine()) is not null)
        {
            var words = line.Split(' ', StringSplitOptions.RemoveEmptyEntries);
            if (words.Length < 2)
            {
                continue;
            }
            if (words[0] == "frames")
            {
                break;
            }
            switch (words[0])
            {
                case "grid" when words.Length >= 3:
                    columns = Whole(words[1]);
                    rows = Whole(words[2]);
                    break;
                case "fps":
                    rate = double.Parse(words[1], CultureInfo.InvariantCulture);
                    break;
                case "light":
                    light = Whole(words[1]);
                    break;
                case "dark":
                    dark = Whole(words[1]);
                    break;
            }
        }
        if (columns <= 0 || rows <= 0 || rate <= 0 || light + dark <= 0)
        {
            throw new InvalidDataException($"{path} is not a film frames.py wrote");
        }
        var film = new Film
        {
            Columns = columns, Rows = rows, FramesASecond = rate, Light = light, Dark = dark,
        };
        while ((line = reader.ReadLine()) is not null)
        {
            var colours = line.Split('|');
            if (colours.Length == 2)
            {
                film.Frames.Add(new Frame(Boxes(colours[0], light), Boxes(colours[1], dark)));
            }
        }
        if (film.Frames.Count == 0)
        {
            throw new InvalidDataException($"{path} has no frames");
        }
        return film;
    }

    private static int Whole(string word) => int.Parse(word, CultureInfo.InvariantCulture);

    /// <summary>One colour's rectangles, leaving out any that names a terminal the film does not have.</summary>
    private static Box[] Boxes(string text, int terminals)
    {
        var boxes = new List<Box>();
        foreach (var each in text.Split(';', StringSplitOptions.RemoveEmptyEntries))
        {
            var n = each.Split(' ', StringSplitOptions.RemoveEmptyEntries);
            if (n.Length == 5 && Whole(n[0]) is var terminal && terminal >= 0 && terminal < terminals)
            {
                boxes.Add(new Box(terminal, Whole(n[1]), Whole(n[2]), Whole(n[3]), Whole(n[4])));
            }
        }
        return boxes.ToArray();
    }
}
