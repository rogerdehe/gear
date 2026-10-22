package decide
import("testing"; "time")
func TestResearchDoingIdleDoneAndBudget(t *testing.T) {
 now:=time.Now(); p:=Policy{MaxWorkers:1,ClaimTimeout:time.Minute,MaxPrompts:2}
 for _,status:=range []string{"idle","done","blocked","unknown","working"}{
 s:=Snapshot{Now:now,Tasks:map[string]Task{"t":{ID:"t",Status:"doing",ClaimedBy:"p"}},Agents:map[string]string{"p":status},Bindings:[]Binding{{TaskID:"t",Pane:"p",PromptedAt:now.Add(-time.Hour),Prompts:100}}}
 actions:=Decide(s,p);t.Logf("task=doing agent=%s prompts=100 configured_max=2 actions=%+v",status,actions)
 if status=="idle" && (len(actions)!=1 || actions[0].Kind!=Prompt){t.Fatal("expected idle prompt")}
 if status!="idle" && len(actions)!=0{t.Fatal("unexpected action")}
 }
}
