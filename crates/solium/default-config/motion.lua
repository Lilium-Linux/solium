-- How this desktop moves: each kind of motion on the curve that fits what
-- it means, rather than one curve for everything.
--
-- user.lua sets the shipped settings from these, and looks.lua, recents.lua,
-- deskstack.lua and overview.lua move with them. Change one here and
-- everything of that kind changes with it; give a single setting its own in
-- user.lua to change just that.
--
-- A spring is `{ easing = { spring = { stiffness, damping, epsilon } } }`,
-- and takes as long as it takes; a damping of twice the square root of the
-- stiffness is the fastest that never overshoots, and these are a touch over
-- that. `epsilon` is how close counts as landed, as a share of the way: the
-- last step, three ten-thousandths, is under a pixel across a whole screen.
-- Anything that moves on a spring and is moved again before it lands
-- carries on from the speed it had.

local motion = {}

-- Travel: the view goes somewhere. A workspace switch, a scrolling column,
-- the next desk in the stack, the next card in Alt+Tab, into the overview.
-- It sets off without a jump, is at full speed within a few frames, and
-- lands softly; pressed again on the way, it turns rather than snapping
-- round. About a third of a second.
motion.travel = { easing = { spring = { stiffness = 600, damping = 49, epsilon = 0.0003 } } }

-- Reflow: windows rearranging where they already are. A tile opening or
-- closing, maximise, fullscreen, a window going back to where it floated,
-- out of the overview. Stiffer: quick and calm, the eye stays on the work.
motion.reflow = { easing = { spring = { stiffness = 750, damping = 55, epsilon = 0.0003 } } }

-- Settle: a window let go after a drag glides into its place. Softer than a
-- reflow, since it starts from the hand.
motion.settle = { easing = { spring = { stiffness = 500, damping = 45, epsilon = 0.0003 } } }

-- Appear: something new arrives, decelerating into its place, and does not
-- overshoot it.
motion.appear = { duration = 260, easing = { 0.1, 0.8, 0.2, 1 } }

-- Fade: a change in place that should not draw the eye, such as how
-- see-through a window is as it loses the keyboard.
motion.fade = { duration = 180, easing = "inOutQuad" }

-- Journey: from one place to another where both ends matter, a window into
-- the taskbar and back out.
motion.journey = { duration = 460, easing = "inOutCubic" }

return motion
