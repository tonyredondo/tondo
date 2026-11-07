# Whether a formula holds of every fair run of a graph, by going through its
# lassos: a run that breaks a formula of a finite graph has a lasso that does.
import json, sys, itertools
cases=json.load(open(sys.argv[1]))
LIMIT=int(sys.argv[2]) if len(sys.argv)>2 else 9
def holds(f, states, loop_at):
    # states: the positions of a lasso, the last going back to loop_at.
    n=len(states)
    succ=lambda i: i+1 if i+1<n else loop_at
    def ev(f):
        if f=='P': return [s in P for s in states]
        if f=='Q': return [s in Q for s in states]
        op=f[0]
        if op=='not': return [not x for x in ev(f[1])]
        if op=='and': a,b=ev(f[1]),ev(f[2]); return [x and y for x,y in zip(a,b)]
        if op=='or': a,b=ev(f[1]),ev(f[2]); return [x or y for x,y in zip(a,b)]
        if op=='next': a=ev(f[1]); return [a[succ(i)] for i in range(n)]
        if op in ('until','eventually','always','leads'):
            if op=='eventually': return ev(['until','T',f[1]])
            if op=='always': return ev(['not',['eventually',['not',f[1]]]])
            if op=='leads': return ev(['always',['or',['not',f[1]],['eventually',f[2]]]])
            a=[True]*n if f[1]=='T' else ev(f[1]); b=ev(f[2])
            res=[False]*n
            for _ in range(2*n+2):
                for i in reversed(range(n)):
                    res[i]=b[i] or (a[i] and res[succ(i)])
            return res
    return ev(f)[0]
out=[]
for c in cases:
    global P,Q
    P=set(c['P']); Q=set(c['Q'])
    adj={s:[] for s in range(c['n'])}
    for a,b,name in c['edges']: adj[a].append((b,name))
    enabled=lambda s,name: any(nm==name for _,nm in adj[s])
    def fair(loop_states, loop_names):
        for a,k in c['fair']:
            taken = a in loop_names
            if k=='weak' and all(enabled(s,a) for s in loop_states) and not taken: return False
            if k=='strong' and any(enabled(s,a) for s in loop_states) and not taken: return False
        return True
    broken=False
    # paths of states with the names of the steps between them
    stack=[([0],[])]
    while stack and not broken:
        path,names=stack.pop()
        last=path[-1]
        moves=adj[last] if adj[last] else [(last,'')]
        for to,name in moves:
            if to in path:
                at=path.index(to)
                loop_states=path[at:]; loop_names=names[at:]+[name]
                if fair(loop_states, set(loop_names)) and not holds(c['f'], path, at):
                    broken=True; break
            if len(path)<LIMIT and to not in path:
                stack.append((path+[to],names+[name]))
            elif len(path)<LIMIT and to in path:
                # going round more than once: unroll the loop once more
                if path.count(to)<2:
                    stack.append((path+[to],names+[name]))
    out.append('fails' if broken else 'holds')
print('\n'.join(out))
