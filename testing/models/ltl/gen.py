# Random graphs, formulas and fairness; the same case for std.model, TLC and
# a brute force over lassos. Writes cases.json, model.to and the TLA specs.
import random, json, sys, os
seed=int(sys.argv[1]); count=int(sys.argv[2]); out=sys.argv[3]; tla_only=len(sys.argv)>4
r=random.Random(seed)
ACTIONS=['a','b','c']
def formula(depth, until):
    if depth==0 or r.random()<0.3:
        return r.choice(['P','Q'])
    ops=['not','and','or','always','eventually','leads']+(['until','next'] if until else [])
    op=r.choice(ops)
    if op in ('not','always','eventually','next'):
        return [op, formula(depth-1, until)]
    if op=='leads':
        return ['leads', r.choice(['P','Q']), r.choice(['P','Q'])]
    return [op, formula(depth-1, until), formula(depth-1, until)]
cases=[]
for i in range(count):
    if r.random()<0.6:
        # A ring that never gets to P, by `a`, with ways out to P from some of
        # its states, by `b` or `c`: what fairness decides.
        ring=r.randint(2,4); n=ring+1
        edges=[(s,(s+1)%ring,'a') for s in range(ring)]
        for s in range(ring):
            if r.random()<0.6:
                edges.append((s,ring,r.choice(['b','c'])))
        if r.random()<0.5:
            edges.append((ring,0,r.choice(ACTIONS)))
        P=[ring]; Q=[s for s in range(n) if r.random()<0.5]
    else:
        n=r.randint(2,5)
        edges=[]
        for s in range(n):
            for t in range(n):
                if s!=t and r.random()<0.5:
                    edges.append((s,t,r.choice(ACTIONS)))
        P=[s for s in range(n) if r.random()<0.5]; Q=[s for s in range(n) if r.random()<0.5]
    until = not tla_only and r.random()<0.5
    # Mostly what comes in the end, which is what fairness changes.
    live=[['eventually','P'],['leads','P','Q'],['always',['eventually','P']],['eventually',['always','Q']],['leads','Q','P']]
    f=r.choice(live) if r.random()<0.7 else formula(3, until)
    fair=[(a, r.choice(['weak','strong'])) for a in ACTIONS if r.random()<0.5]
    cases.append(dict(n=n,edges=edges,P=P,Q=Q,f=f,fair=fair,until=until))
json.dump(cases, open(os.path.join(out,'cases.json'),'w'))
# std.model
def tondo(f):
    if f=='P': return 'model.state((s: Int) { p.contains(s) })'
    if f=='Q': return 'model.state((s: Int) { q.contains(s) })'
    op=f[0]
    if op=='not': return '%s.negated()'%tondo(f[1])
    if op=='always': return '%s.always()'%tondo(f[1])
    if op=='eventually': return '%s.eventually()'%tondo(f[1])
    if op=='next': return '%s.next()'%tondo(f[1])
    if op=='and': return 'model.allOf(%s, %s)'%(tondo(f[1]),tondo(f[2]))
    if op=='or': return 'model.anyOf(%s, %s)'%(tondo(f[1]),tondo(f[2]))
    if op=='until': return '%s.until(%s)'%(tondo(f[1]),tondo(f[2]))
    if op=='leads': return '%s.implies(%s.eventually()).always()'%(tondo(f[1]),tondo(f[2]))
lines=['import std.console','import std.model','']
for i,c in enumerate(cases):
    lines.append('fn steps%d(s: Int): Array[model.Step[Int]] {'%i)
    lines.append('    var next: Array[model.Step[Int]] = []')
    for (a,b,name) in c['edges']:
        lines.append('    if s == %d { next.push(model.Step { name: "%s", state: %d }) }'%(a,name,b))
    lines.append('    next')
    lines.append('}')
    lines.append('')
    lines.append('fn case%d(): String {'%i)
    lines.append('    let p = [%s]'%', '.join(map(str,c['P'])) if c['P'] else '    let p: Array[Int] = []')
    lines.append('    let q = [%s]'%', '.join(map(str,c['Q'])) if c['Q'] else '    let q: Array[Int] = []')
    fair=', '.join('model.%s("%s")'%(k,a) for a,k in c['fair'])
    lines.append('    match model.check(0, steps%d, holds: [model.temporal(%s)], fair: [%s]) {'%(i,tondo(c['f']),fair))
    lines.append('        ok(_) => "holds"')
    lines.append('        err(_) => "fails"')
    lines.append('    }')
    lines.append('}')
    lines.append('')
lines.append('fn main() {')
for i in range(len(cases)):
    lines.append('    console.println("{case%d()}")'%i)
lines.append('}')
open(os.path.join(out,'model.to'),'w').write('\n'.join(lines)+'\n')
# TLA+, for the cases with no until or next
def tla(f):
    if f=='P': return 'P'
    if f=='Q': return 'Q'
    op=f[0]
    if op=='not': return '~(%s)'%tla(f[1])
    if op=='always': return '[](%s)'%tla(f[1])
    if op=='eventually': return '<>(%s)'%tla(f[1])
    if op=='and': return '(%s) /\\ (%s)'%(tla(f[1]),tla(f[2]))
    if op=='or': return '(%s) \\/ (%s)'%(tla(f[1]),tla(f[2]))
    if op=='leads': return '(%s) ~> (%s)'%(tla(f[1]),tla(f[2]))
os.makedirs(os.path.join(out,'tla'),exist_ok=True)
for i,c in enumerate(cases):
    if c['until']: continue
    acts=[]
    for a in ACTIONS:
        es=[e for e in c['edges'] if e[2]==a]
        body=' \\/ '.join('(s = %d /\\ s\' = %d)'%(x,y) for x,y,_ in es) if es else 'FALSE'
        acts.append('%s == %s'%(a.upper(), body))
    fairs=' /\\ '.join(('WF_s(%s)' if k=='weak' else 'SF_s(%s)')%a.upper() for a,k in c['fair'])
    spec='---- MODULE G%d ----\nEXTENDS Naturals\nVARIABLE s\nP == s \\in {%s}\nQ == s \\in {%s}\nInit == s = 0\n%s\nNext == A \\/ B \\/ C\nSpec == Init /\\ [][Next]_s /\\ WF_s(Next)%s\nProp == %s\n====\n'%(
        i, ', '.join(map(str,c['P'])), ', '.join(map(str,c['Q'])), '\n'.join(acts), (' /\\ '+fairs) if fairs else '', tla(c['f']))
    open(os.path.join(out,'tla','G%d.tla'%i),'w').write(spec)
    open(os.path.join(out,'tla','G%d.cfg'%i),'w').write('SPECIFICATION Spec\nPROPERTY Prop\nCHECK_DEADLOCK FALSE\n')
