// GM Tools and Redeem Code hub buttons for Free Realms Expanded. Adapted from Sanctuary PR #163's
// gm_hub_button.as (BubbaJoeX, AGPL-3.0): two more hub trees right of the Marketplace icon. GM Tools
// sends "!gm" (the server opens the GM panel for accounts allowed to use it); Redeem Code sends
// "!redeem" (the chat window's redeem box). Instead of recoloured copies of treeIcon built into the
// movie, the stock treeIcon is tinted here (purple and gold). Compiled into its own DoAction after
// gamedock's frame 1 script.

function gmTint(clip, r, g, b)
{
   // Keep the tree's shading: scale each channel and add a little of the target colour.
   new Color(clip).setTransform({ra: 55, rb: r, ga: 55, gb: g, ba: 55, bb: b, aa: 100, ab: 0});
}

function gmHubIcon(name, depth, tooltip, command, beside)
{
   var b = _root.attachMovie("treeIcon", name, depth);
   b.setTooltipStr(tooltip);

   // Line it up with the icon to its left, using on-stage bounds so it works with any registration point.
   // (No rescale: GameDockIcon's hover tween resets _xscale/_yscale.)
   var left = beside.getBounds(_root);
   var own = b.getBounds(_root);
   b._x += left.xMax + 8 - own.xMin;
   b._y += (left.yMin + left.yMax) / 2 - (own.yMin + own.yMax) / 2;

   b.onPress = function()
   {
      this.onRollOut();
      fscommand("FR_event", "PlaySound(DockSelect)");
      fscommand("FR_event", "sendChatMessage(  \"" + command + "\")");
   };
   b.onRelease = b.onReleaseOutside = function()
   {
   };
   return b;
}

gmTint(gmHubIcon("gmHubButton", 9000, "GM Tools", "!gm", shopButton), 110, 20, 150);
gmTint(gmHubIcon("gmRedeemButton", 9001, "Redeem Code", "!redeem", _root.gmHubButton), 150, 110, 0);

// Lua shows and hides the hub icons through showDockIcons_lua; keep ours in step.
var gmOriginalShowDockIcons = showDockIcons_lua;
showDockIcons_lua = function(n)
{
   gmOriginalShowDockIcons.apply(_root, arguments);
   _root.gmHubButton._visible = n == 1;
   _root.gmRedeemButton._visible = n == 1;
};
