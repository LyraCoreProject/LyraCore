s/#([A-Za-z_][A-Za-z0-9_]*)/table.getn(\1)/g
s/return select\("#", \.\.\.\)/return arg.n/
s/if __TS__CountVarargs\(\.\.\.\) ~= 0/if arg.n ~= 0/
s/accumulator = \.\.\./accumulator = arg[1]/
s/instance:____constructor\(\.\.\.\)/instance:____constructor(unpack(arg))/
